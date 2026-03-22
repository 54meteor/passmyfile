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

use crate::types::{TransferTask, TransferDirection, TransferStatus, PendingFileRequest, FolderTransferMeta, FolderFileInfo};
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
    is_folder: bool,
    folder_meta: Option<FolderTransferMeta>,
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
                            
                            // 判断是文件还是文件夹
                            if meta_str.starts_with("FOLDER:") {
                                // 文件夹传输协议
                                Self::handle_folder_receive(
                                    stream, addr.ip().to_string(), 
                                    meta_str.to_string(), downloads_dir, tasks, progress_tx,
                                    pending_connections, pending_requests
                                ).await;
                            } else if meta_str.starts_with("FILE:") {
                                // 单文件传输
                                let parts: Vec<&str> = meta_str.split(':').collect();
                                if parts.len() < 3 {
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
                                        is_folder: false,
                                        folder_meta: None,
                                    });
                                }

                                // 添加到待处理请求
                                let pending = PendingFileRequest {
                                    id: request_id.clone(),
                                    file_name: file_name.clone(),
                                    file_size,
                                    sender_ip: sender_ip.clone(),
                                    is_folder: false,
                                    file_count: 1,
                                    total_size: file_size,
                                };
                                {
                                    let mut reqs = pending_requests.write().await;
                                    reqs.push(pending);
                                }

                                write_log(&format!("File incoming, waiting for confirmation: {} ({} bytes) from {}", file_name, file_size, sender_ip));
                            }
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

    /// 处理文件夹接收
    async fn handle_folder_receive(
        mut stream: TcpStream,
        sender_ip: String,
        meta_str: String,
        downloads_dir: PathBuf,
        tasks: Arc<RwLock<HashMap<String, TransferTask>>>,
        progress_tx: broadcast::Sender<TransferProgress>,
        pending_connections: Arc<RwLock<HashMap<String, PendingConnection>>>,
        pending_requests: Arc<RwLock<Vec<PendingFileRequest>>>,
    ) {
        // 解析文件夹元信息: FOLDER:{folder_name}:{total_files}:{total_size}
        let parts: Vec<&str> = meta_str.split(':').collect();
        if parts.len() < 4 {
            write_log("Invalid folder meta format");
            return;
        }
        
        let folder_name = parts[1].to_string();
        let total_files: u32 = parts[2].parse().unwrap_or(0);
        let total_size: u64 = parts[3].parse().unwrap_or(0);
        
        write_log(&format!("Folder incoming: {} ({} files, {} bytes) from {}", 
            folder_name, total_files, total_size, sender_ip));
        
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
                file_name: folder_name.clone(),
                file_size: total_size,
                sender_ip: sender_ip.clone(),
                created_at: chrono::Utc::now().timestamp(),
                is_folder: true,
                folder_meta: Some(FolderTransferMeta {
                    folder_name: folder_name.clone(),
                    total_files,
                    total_size,
                    files: Vec::new(),
                }),
            });
        }
        
        // 添加到待处理请求
        let pending = PendingFileRequest {
            id: request_id.clone(),
            file_name: folder_name.clone(),
            file_size: total_size,
            sender_ip: sender_ip.clone(),
            is_folder: true,
            file_count: total_files,
            total_size,
        };
        {
            let mut reqs = pending_requests.write().await;
            reqs.push(pending);
        }
        
        write_log(&format!("Folder incoming, waiting for confirmation: {}", folder_name));
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

        let file_name = conn.file_name.clone(); // 保存文件名用于最后的日志
        if conn.is_folder {
            // 接收文件夹
            Self::receive_folder(&mut stream, save_dir, conn.file_name, task_id.clone(), conn.file_size, self.progress_tx.clone()).await?;
        } else {
            // 接收单个文件
            Self::receive_file(&mut stream, save_dir, conn.file_name, task_id.clone(), conn.file_size, self.progress_tx.clone()).await?;
        }

        // 更新任务状态
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.get_mut(&task_id) {
                task.transferred = conn.file_size;
                task.progress = 100.0;
                task.status = TransferStatus::Completed;
            }
        }

        let _ = self.progress_tx.send(TransferProgress {
            task_id,
            transferred: conn.file_size,
            progress: 100.0,
            speed: 0,
            status: TransferStatus::Completed,
        });

        write_log(&format!("Transfer completed: {}", file_name));

        Ok(())
    }

    /// 接收文件夹
    async fn receive_folder(
        stream: &mut TcpStream,
        save_dir: PathBuf,
        folder_name: String,
        task_id: String,
        total_size: u64,
        progress_tx: broadcast::Sender<TransferProgress>,
    ) -> Result<()> {
        let folder_path = save_dir.join(&folder_name);
        std::fs::create_dir_all(&folder_path)?;
        
        let mut total_received: u64 = 0;
        
        // 读取文件列表: 每个文件一行 "FILE:{relative_path}:{size}"
        loop {
            let mut line_buf = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                let n = stream.read(&mut byte).await?;
                if n == 0 || byte[0] == b'\n' {
                    break;
                }
                line_buf.push(byte[0]);
            }
            
            let line = String::from_utf8_lossy(&line_buf);
            if line.trim().is_empty() {
                continue;
            }
            
            if line.starts_with("END") {
                break;
            }
            
            if !line.starts_with("FILE:") {
                continue;
            }
            
            // 解析: FILE:{relative_path}:{size}
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 3 {
                continue;
            }
            
            let relative_path = parts[1].to_string();
            let file_size: u64 = parts[2].parse().unwrap_or(0);
            
            // 创建文件的父目录
            let file_path = folder_path.join(&relative_path);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            
            write_log(&format!("Receiving file: {} ({} bytes)", relative_path, file_size));
            
            // 接收文件数据
            let mut file = File::create(&file_path)?;
            let mut buf = vec![0u8; CHUNK_SIZE];
            let mut file_received: u64 = 0;
            
            while file_received < file_size {
                let to_read = std::cmp::min(CHUNK_SIZE, (file_size - file_received) as usize);
                let n = stream.read(&mut buf[..to_read]).await?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])?;
                file_received += n as u64;
                total_received += n as u64;
                
                let progress = (total_received as f32 / total_size as f32) * 100.0;
                let _ = progress_tx.send(TransferProgress {
                    task_id: task_id.clone(),
                    transferred: total_received,
                    progress,
                    speed: 0,
                    status: TransferStatus::Transferring,
                });
            }
        }
        
        write_log(&format!("Folder received: {}", folder_name));
        Ok(())
    }

    /// 接收单个文件
    async fn receive_file(
        stream: &mut TcpStream,
        save_dir: PathBuf,
        file_name: String,
        task_id: String,
        file_size: u64,
        progress_tx: broadcast::Sender<TransferProgress>,
    ) -> Result<()> {
        let file_path = save_dir.join(&file_name);
        let mut file = File::create(&file_path)?;
        let mut total_received: u64 = 0;
        let mut buf = vec![0u8; CHUNK_SIZE];

        loop {
            let to_read = std::cmp::min(CHUNK_SIZE, (file_size - total_received) as usize);
            if to_read == 0 { break; }

            let n = match stream.read(&mut buf[..to_read]).await {
                Ok(n) if n > 0 => n,
                _ => break,
            };

            if file.write_all(&buf[..n]).is_err() {
                break;
            }
            total_received += n as u64;

            let progress = (total_received as f32 / file_size as f32) * 100.0;
            let _ = progress_tx.send(TransferProgress {
                task_id: task_id.clone(),
                transferred: total_received,
                progress,
                speed: 0,
                status: TransferStatus::Transferring,
            });
        }
        
        write_log(&format!("File received: {} ({} bytes)", file_name, total_received));
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

    /// 发送文件到指定设备（支持文件和文件夹）
    pub async fn send_file(&self, target_ip: String, target_port: u16, file_path: PathBuf) -> Result<String> {
        let metadata = std::fs::metadata(&file_path)?;
        
        // 判断是文件还是文件夹
        if metadata.is_dir() {
            self.send_folder(target_ip, target_port, file_path).await
        } else {
            self.send_single_file(target_ip, target_port, file_path).await
        }
    }

    /// 发送文件夹
    pub async fn send_folder(&self, target_ip: String, target_port: u16, folder_path: PathBuf) -> Result<String> {
        let folder_name = folder_path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        
        // 扫描文件夹获取所有文件
        let mut files = Vec::new();
        let mut total_size: u64 = 0;
        Self::scan_folder(&folder_path, &folder_path, &mut files, &mut total_size)?;
        
        write_log(&format!("Folder scan complete: {} files, {} total bytes", files.len(), total_size));
        
        let task_id = uuid::Uuid::new_v4().to_string();
        
        // 连接到接收方
        let mut stream = tokio::net::TcpStream::connect(format!("{}:{}", target_ip, target_port)).await?;
        
        // 发送文件夹元信息: FOLDER:{folder_name}:{total_files}:{total_size}
        let meta = format!("FOLDER:{}:{}:{}\n", folder_name, files.len(), total_size);
        stream.write_all(meta.as_bytes()).await?;
        
        // 等待确认
        let mut ack_buf = vec![0u8; 1024];
        let n = stream.read(&mut ack_buf).await?;
        ack_buf.truncate(n);
        
        if !ack_buf.starts_with(b"ACK") {
            return Err(anyhow!("传输被拒绝"));
        }
        
        // 发送文件列表
        for file_info in &files {
            let file_line = format!("FILE:{}:{}\n", file_info.relative_path, file_info.size);
            stream.write_all(file_line.as_bytes()).await?;
        }
        stream.write_all(b"END\n").await?;
        
        // 等待接收方准备就绪
        let mut ready_buf = vec![0u8; 1024];
        let n = stream.read(&mut ready_buf).await?;
        ready_buf.truncate(n);
        
        if &ready_buf != b"READY" {
            return Err(anyhow!("接收方未准备就绪"));
        }
        
        info!("Starting folder transfer: {} to {}:{}", folder_name, target_ip, target_port);
        
        // 创建任务
        let task = TransferTask {
            id: task_id.clone(),
            file_name: folder_name.clone(),
            file_size: total_size,
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
        
        // 发送每个文件的数据
        let mut transferred: u64 = 0;
        for file_info in &files {
            let full_path = folder_path.join(&file_info.relative_path);
            write_log(&format!("Sending file: {}", file_info.relative_path));
            
            let mut file = File::open(&full_path)?;
            let mut buf = vec![0u8; CHUNK_SIZE];
            
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                stream.write_all(&buf[..n]).await?;
                transferred += n as u64;
                
                let progress = (transferred as f32 / total_size as f32) * 100.0;
                let _ = self.progress_tx.send(TransferProgress {
                    task_id: task_id.clone(),
                    transferred,
                    progress,
                    speed: 0,
                    status: TransferStatus::Transferring,
                });
            }
        }
        
        // 更新任务状态
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.get_mut(&task_id) {
                task.transferred = total_size;
                task.progress = 100.0;
                task.status = TransferStatus::Completed;
            }
        }
        
        let _ = self.progress_tx.send(TransferProgress {
            task_id: task_id.clone(),
            transferred: total_size,
            progress: 100.0,
            speed: 0,
            status: TransferStatus::Completed,
        });
        
        info!("Folder sent: {} ({} bytes)", folder_name, transferred);
        
        Ok(task_id)
    }

    /// 扫描文件夹获取所有文件
    fn scan_folder(base_path: &PathBuf, current_path: &PathBuf, files: &mut Vec<FolderFileInfo>, total_size: &mut u64) -> Result<()> {
        for entry in std::fs::read_dir(current_path)? {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_dir() {
                Self::scan_folder(base_path, &path, files, total_size)?;
            } else {
                let metadata = std::fs::metadata(&path)?;
                let relative_path = path.strip_prefix(base_path)
                    .map(|p| p.to_string_lossy().to_string().replace("\\", "/"))
                    .unwrap_or_else(|_| path.file_name().unwrap().to_string_lossy().to_string());
                
                *total_size += metadata.len();
                files.push(FolderFileInfo {
                    relative_path,
                    size: metadata.len(),
                });
            }
        }
        Ok(())
    }

    /// 发送单个文件
    async fn send_single_file(&self, target_ip: String, target_port: u16, file_path: PathBuf) -> Result<String> {
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
