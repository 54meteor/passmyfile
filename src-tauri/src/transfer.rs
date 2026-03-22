use anyhow::{Result, anyhow};
use log::{info, error};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, RwLock};

use crate::types::{TransferTask, TransferDirection, TransferStatus, PendingFileRequest};
use crate::write_log;

const TRANSFER_PORT: u16 = 18766;
const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks

/// 传输进度更新
#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub task_id: String,
    pub transferred: u64,
    pub progress: f32,
    pub speed: u64,
    pub status: TransferStatus,
}

/// 待处理的连接（等待用户确认）
struct PendingConnection {
    stream: TcpStream,
    file_name: String,
    file_size: u64,
    sender_ip: String,
    created_at: i64,
}

/// 文件传输服务
pub struct TransferService {
    tcp_port: u16,
    downloads_dir: PathBuf,
    tasks: Arc<RwLock<HashMap<String, TransferTask>>>,
    progress_tx: broadcast::Sender<TransferProgress>,
    /// 待处理连接（等待用户确认）
    pending_connections: Arc<RwLock<HashMap<String, PendingConnection>>>,
    /// 待接收文件请求列表（发送给前端）
    pending_requests: Arc<RwLock<Vec<PendingFileRequest>>>,
}

impl TransferService {
    /// 创建传输服务
    pub fn new(_device_id: String, downloads_dir: PathBuf) -> Self {
        let (progress_tx, _) = broadcast::channel(100);

        Self {
            tcp_port: TRANSFER_PORT,
            downloads_dir,
            tasks: Arc::new(RwLock::new(HashMap::new())),
            progress_tx,
            pending_connections: Arc::new(RwLock::new(HashMap::new())),
            pending_requests: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 启动传输服务
    pub async fn start(&mut self) -> Result<()> {
        std::fs::create_dir_all(&self.downloads_dir)?;

        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", self.tcp_port)).await?;

        write_log(&format!("Transfer service started on port {}", self.tcp_port));
        info!("Transfer service started on port {}", self.tcp_port);

        let downloads_dir = self.downloads_dir.clone();
        let tasks = self.tasks.clone();
        let progress_tx = self.progress_tx.clone();
        let pending_connections = self.pending_connections.clone();
        let pending_requests = self.pending_requests.clone();

        // 启动清理过期连接的定时任务
        let pending_conn_for_cleanup = pending_connections.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

                let now = chrono::Utc::now().timestamp();
                let mut to_remove = Vec::new();

                {
                    let mut conns = pending_conn_for_cleanup.write().await;
                    for (id, conn) in conns.iter() {
                        if now - conn.created_at > 60 {
                            to_remove.push(id.clone());
                        }
                    }
                    for id in &to_remove {
                        conns.remove(id);
                        write_log(&format!("Pending connection expired: {}", id));
                    }
                }
            }
        });

        // 处理连接
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((mut stream, addr)) => {
                        let downloads_dir = downloads_dir.clone();
                        let tasks = tasks.clone();
                        let progress_tx = progress_tx.clone();
                        let pending_connections = pending_connections.clone();
                        let pending_requests = pending_requests.clone();

                        tokio::spawn(async move {
                            // 读取元信息
                            let mut meta_buf = [0u8; 1024];
                            let n = match stream.read(&mut meta_buf).await {
                                Ok(n) if n > 0 => n,
                                _ => return,
                            };

                            let meta_str = String::from_utf8_lossy(&meta_buf[..n]);
                            let parts: Vec<&str> = meta_str.split(':').collect();
                            if parts.len() < 3 || parts[0] != "FILE" {
                                return;
                            }

                            let file_name = parts[1].to_string();
                            let file_size: u64 = parts[2].parse().unwrap_or(0);
                            let sender_ip = addr.ip().to_string();

                            // 先发送 ACK 保持连接，等待用户确认
                            if let Err(e) = stream.write_all(b"ACK").await {
                                write_log(&format!("Failed to send initial ACK: {}", e));
                                return;
                            }

                            let request_id = uuid::Uuid::new_v4().to_string();

                            // 保存连接，等待用户确认
                            {
                                let mut conns = pending_connections.write().await;
                                conns.insert(request_id.clone(), PendingConnection {
                                    stream,
                                    file_name: file_name.clone(),
                                    file_size,
                                    sender_ip: sender_ip.clone(),
                                    created_at: chrono::Utc::now().timestamp(),
                                });
                            }

                            // 添加到待处理请求
                            let pending = PendingFileRequest {
                                id: request_id.clone(),
                                file_name: file_name.clone(),
                                file_size,
                                sender_ip: sender_ip.clone(),
                            };
                            {
                                let mut reqs = pending_requests.write().await;
                                reqs.push(pending);
                            }

                            write_log(&format!("File incoming, waiting for confirmation: {} ({} bytes) from {}", file_name, file_size, sender_ip));
                        });
                    }
                    Err(e) => {
                        error!("Accept error: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            }
        });

        Ok(())
    }

    /// 获取待接收文件请求列表
    pub async fn get_pending_requests(&self) -> Vec<PendingFileRequest> {
        let reqs = self.pending_requests.read().await;
        reqs.clone()
    }

    /// 确认接收文件（用户点击接收）
    pub async fn confirm_receive(&self, request_id: String, save_path: Option<String>) -> Result<()> {
        let conn = {
            let mut conns = self.pending_connections.write().await;
            conns.remove(&request_id)
        };

        let mut reqs = self.pending_requests.write().await;
        reqs.retain(|r| r.id != request_id);

        let conn = conn.ok_or_else(|| anyhow!("请求已过期"))?;

        let save_dir = if let Some(ref path) = save_path {
            PathBuf::from(path)
        } else {
            self.downloads_dir.clone()
        };
        std::fs::create_dir_all(&save_dir)?;

        // 发送 READY 信号表示用户已确认
        let mut stream = conn.stream;
        stream.write_all(b"READY").await?;

        write_log(&format!("User confirmed, starting to receive: {}", conn.file_name));

        // 创建任务
        let task_id = request_id.clone();
        let task = TransferTask {
            id: task_id.clone(),
            file_name: conn.file_name.clone(),
            file_size: conn.file_size,
            transferred: 0,
            progress: 0.0,
            speed: 0,
            direction: TransferDirection::Receive,
            status: TransferStatus::Transferring,
            peer_id: conn.sender_ip.clone(),
            peer_name: format!("主机 ({})", conn.sender_ip),
        };

        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id.clone(), task);
        }

        // 接收文件
        let file_path = save_dir.join(&conn.file_name);
        let mut file = File::create(&file_path)?;
        let mut total_received: u64 = 0;
        let mut buf = vec![0u8; CHUNK_SIZE];

        loop {
            let to_read = std::cmp::min(CHUNK_SIZE, (conn.file_size - total_received) as usize);
            if to_read == 0 { break; }

            let n = match stream.read(&mut buf[..to_read]).await {
                Ok(n) if n > 0 => n,
                _ => break,
            };

            if file.write_all(&buf[..n]).is_err() {
                break;
            }
            total_received += n as u64;

            let progress = (total_received as f32 / conn.file_size as f32) * 100.0;
            let _ = self.progress_tx.send(TransferProgress {
                task_id: task_id.clone(),
                transferred: total_received,
                progress,
                speed: 0,
                status: TransferStatus::Transferring,
            });
        }

        // 更新任务状态
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.get_mut(&task_id) {
                task.transferred = total_received;
                task.progress = 100.0;
                task.status = TransferStatus::Completed;
            }
        }

        let _ = self.progress_tx.send(TransferProgress {
            task_id,
            transferred: total_received,
            progress: 100.0,
            speed: 0,
            status: TransferStatus::Completed,
        });

        write_log(&format!("File received: {} ({} bytes)", conn.file_name, total_received));

        Ok(())
    }

    /// 拒绝接收文件
    pub async fn reject_receive(&self, request_id: String) -> Result<()> {
        let conn = {
            let mut conns = self.pending_connections.write().await;
            conns.remove(&request_id)
        };

        let mut reqs = self.pending_requests.write().await;
        reqs.retain(|r| r.id != request_id);

        if let Some(conn) = conn {
            // 发送拒绝并关闭
            let mut stream = conn.stream;
            let _ = stream.write_all(b"REJECT").await;
            write_log(&format!("Rejected file: {}", conn.file_name));
        }

        Ok(())
    }

    /// 发送文件到指定设备
    pub async fn send_file(&self, target_ip: String, target_port: u16, file_path: PathBuf) -> Result<String> {
        let file = File::open(&file_path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        let file_name = file_path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let task_id = uuid::Uuid::new_v4().to_string();

        // 连接到接收方
        let mut stream = tokio::net::TcpStream::connect(format!("{}:{}", target_ip, target_port)).await?;

        // 发送文件元信息
        let meta = format!("FILE:{}:{}", file_name, file_size);
        stream.write_all(meta.as_bytes()).await?;

        // 等待确认
        let mut ack_buf = vec![0u8; 1024];
        let n = stream.read(&mut ack_buf).await?;
        ack_buf.truncate(n);

        if &ack_buf == b"REJECT" {
            return Err(anyhow!("对方拒绝了文件接收"));
        }

        if !ack_buf.starts_with(b"ACK") {
            return Err(anyhow!("传输被拒绝"));
        }

        // 等待接收方准备就绪
        write_log("Waiting for receiver to confirm...");
        let mut ready_buf = vec![0u8; 1024];
        let n = stream.read(&mut ready_buf).await?;
        ready_buf.truncate(n);

        if &ready_buf != b"READY" {
            return Err(anyhow!("接收方未准备就绪, got: {:?}", String::from_utf8_lossy(&ready_buf)));
        }

        info!("Starting file transfer: {} to {}:{}", file_name, target_ip, target_port);

        // 创建任务
        let task = TransferTask {
            id: task_id.clone(),
            file_name: file_name.clone(),
            file_size,
            transferred: 0,
            progress: 0.0,
            speed: 0,
            direction: TransferDirection::Send,
            status: TransferStatus::Transferring,
            peer_id: target_ip.clone(),
            peer_name: format!("主机 ({})", target_ip),
        };

        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id.clone(), task);
        }

        // 发送文件数据
        let mut file = File::open(&file_path)?;
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut transferred: u64 = 0;

        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            stream.write_all(&buf[..n]).await?;
            transferred += n as u64;

            let progress = (transferred as f32 / file_size as f32) * 100.0;
            let _ = self.progress_tx.send(TransferProgress {
                task_id: task_id.clone(),
                transferred,
                progress,
                speed: 0,
                status: TransferStatus::Transferring,
            });
        }

        // 更新任务状态
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.get_mut(&task_id) {
                task.transferred = file_size;
                task.progress = 100.0;
                task.status = TransferStatus::Completed;
            }
        }

        let _ = self.progress_tx.send(TransferProgress {
            task_id: task_id.clone(),
            transferred: file_size,
            progress: 100.0,
            speed: 0,
            status: TransferStatus::Completed,
        });

        info!("File sent: {} ({} bytes)", file_name, transferred);

        Ok(task_id)
    }

    /// 获取传输任务列表
    pub async fn get_tasks(&self) -> Vec<TransferTask> {
        let tasks = self.tasks.read().await;
        tasks.values().cloned().collect()
    }
}
