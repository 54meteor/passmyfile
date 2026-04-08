use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::fs::OpenOptions;
use std::io::Write;

mod types;
mod discovery;
mod transfer;
mod udp_discovery;
mod http_server;

use types::{Device, TransferTask, AppState, SharedDir, HostInfo};
use discovery::DiscoveryService;
use transfer::TransferService;
use udp_discovery::UdpDiscoveryService;
use http_server::HttpFileServer;

/// 过滤路径遍历字符，只保留安全的文件名部分
fn sanitize_path_component(name: &str) -> String {
    // 移除所有 .. 路径遍历序列
    let name = name.replace("..", "_");
    // 取最后一个路径组件（防止 123.jpg/something 类型的绕过）
    std::path::Path::new(&name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string())
}

/// 过滤并规范化相对路径，移除 .. 和 . 组件，验证不超出 base
fn sanitize_rel_path(rel_path: &str, base: &std::path::Path) -> Result<std::path::PathBuf, String> {
    // 使用 components() 正确解析路径，跳过 Prefix(C:) 这种 Windows 盘符前缀
    let mut result = std::path::PathBuf::new();
    for comp in std::path::Path::new(rel_path).components() {
        match comp {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            // Normal 可能是普通文件名，也可能是裸盘符如 "D:"（Windows 上 components() 会把 "D:" 解析为 Prefix + Relative）
            std::path::Component::Normal(name) => {
                let n = name.to_string_lossy();
                // 裸盘符如 "D:" 跳过（会导致 join 行为异常）
                if n.len() == 2 && n.chars().nth(1) == Some(':') {
                    continue;
                }
                result.push(name);
            }
            // 跳过 Prefix（如 Windows 盘符前缀 C:) 和 RootDir
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                continue;
            }
        }
    }
    let full = base.join(&result);
    // 验证最终路径不超出 base
    let full_canon = full.canonicalize().unwrap_or(full.clone());
    let base_canon = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    if !full_canon.starts_with(&base_canon) {
        return Err(format!("路径遍历被拒绝: {:?}", result));
    }
    Ok(result)
}

pub fn write_log(msg: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("feige-transfer.log")
    {
        let _ = writeln!(file, "[{}] {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"), msg);
    }
}

/// 应用状态管理器
pub struct AppManager {
    state: Arc<RwLock<AppState>>,
    discovery: Option<DiscoveryService>,
    transfer: Option<TransferService>,
    udp_discovery: Option<UdpDiscoveryService>,
    http_server: Option<Arc<HttpFileServer>>,
}

impl AppManager {
    pub fn new() -> Self {
        let state = AppState::default();
        
        Self {
            state: Arc::new(RwLock::new(state)),
            discovery: None,
            transfer: None,
            udp_discovery: None,
            http_server: None,
        }
    }

    /// 初始化服务
    pub async fn init(&mut self) -> Result<(), String> {
        let state = self.state.read().await;
        
        // 创建下载目录
        let downloads = dirs::download_dir().unwrap_or_else(|| PathBuf::from("."));
        let downloads_dir = downloads.join("FeigeTransfer");
        std::fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
        
        // 初始化发现服务（简化版）
        let discovery = DiscoveryService::new(
            state.device_id.clone(),
            state.device_name.clone(),
            18766,
        ).map_err(|e| e.to_string())?;
        
        // 初始化传输服务
        let mut transfer = TransferService::new(
            state.device_id.clone(),
            downloads_dir.clone(),
        );
        
        // 启动服务
        discovery.start().await;
        transfer.start().await.map_err(|e| e.to_string())?;
        
        // 初始化 HTTP 文件服务器
        let http_server = HttpFileServer::new(
            downloads_dir,
            state.device_id.clone(),
            state.device_name.clone(),
        );
        http_server.start().await.map_err(|e| e.to_string())?;
        
        self.discovery = Some(discovery);
        self.transfer = Some(transfer);
        self.http_server = Some(Arc::new(http_server));
        
        write_log("[FeigeTransfer] Services started");
        Ok(())
    }

    /// 获取设备列表
    pub async fn get_devices(&self) -> Vec<Device> {
        if let Some(ref discovery) = self.discovery {
            discovery.get_devices().await
        } else {
            Vec::new()
        }
    }

    /// 获取传输任务列表
    pub async fn get_transfers(&self) -> Vec<TransferTask> {
        if let Some(ref transfer) = self.transfer {
            transfer.get_tasks().await
        } else {
            Vec::new()
        }
    }

    /// 获取设备名称
    pub async fn get_device_name(&self) -> String {
        let state = self.state.read().await;
        state.device_name.clone()
    }

    /// 设置设备名称
    pub async fn set_device_name(&mut self, name: String) {
        let mut state = self.state.write().await;
        state.device_name = name;
    }

    /// 获取设备ID
    pub async fn get_device_id(&self) -> String {
        let state = self.state.read().await;
        state.device_id.clone()
    }

    /// 添加对等主机
    pub async fn add_peer(&self, peer_ip: String, peer_port: u16) -> Result<Device, String> {
        if let Some(ref discovery) = self.discovery {
            discovery.add_peer(peer_ip, peer_port).await.map_err(|e| e.to_string())
        } else {
            Err("Discovery service not initialized".to_string())
        }
    }

    /// 扫描子网发现主机
    pub async fn scan_subnet(&self, port: u16) -> Vec<Device> {
        if let Some(ref discovery) = self.discovery {
            discovery.scan_subnet(port).await
        } else {
            Vec::new()
        }
    }

    /// 获取本机 IP
    pub fn get_local_ip(&self) -> String {
        if let Some(ref discovery) = self.discovery {
            discovery.get_local_ip_str()
        } else {
            "未知".to_string()
        }
    }

    /// 获取本机端口
    pub fn get_local_port(&self) -> u16 {
        if let Some(ref discovery) = self.discovery {
            discovery.get_local_port()
        } else {
            18766
        }
    }

    /// 获取待接收文件请求
    pub async fn get_pending_requests(&self) -> Vec<crate::types::PendingFileRequest> {
        if let Some(ref transfer) = self.transfer {
            transfer.get_pending_requests().await
        } else {
            Vec::new()
        }
    }

    /// 确认接收文件
    pub async fn confirm_receive(&self, request_id: String, save_path: Option<String>) -> Result<(), String> {
        if let Some(ref transfer) = self.transfer {
            transfer.confirm_receive(request_id, save_path).await.map_err(|e| e.to_string())
        } else {
            Err("Transfer service not initialized".to_string())
        }
    }

    /// 拒绝接收文件
    pub async fn reject_receive(&self, request_id: String) -> Result<(), String> {
        if let Some(ref transfer) = self.transfer {
            transfer.reject_receive(request_id).await.map_err(|e| e.to_string())
        } else {
            Err("Transfer service not initialized".to_string())
        }
    }

    /// ============ UDP 发现服务 ============

    /// 启动 UDP 发现服务（Host 模式）
    pub async fn start_udp_discovery(&mut self, name: String, port: u16, shared_dirs: Vec<SharedDir>) -> Result<(), String> {
        write_log("[AppManager] start_udp_discovery: entering");
        let device_id = {
            let state = self.state.read().await;
            state.device_id.clone()
        };
        write_log(&format!("[AppManager] start_udp_discovery: device_id={}", device_id));

        // 创建一个实例，broadcast 和 listener 克隆 shared 的 Arc
        let udp = UdpDiscoveryService::new(device_id.clone(), name.clone(), port);
        write_log("[AppManager] start_udp_discovery: UdpDiscoveryService created");
        
        // 先更新共享目录（同步到所有克隆）
        let udp_clone = udp.clone();
        tokio::spawn(async move {
            udp_clone.update_shared_dirs(shared_dirs).await;
        });
        write_log("[AppManager] start_udp_discovery: update_shared_dirs spawned");

        // 启动广播任务
        let udp_for_broadcast = udp.clone();
        tokio::spawn(async move {
            udp_for_broadcast.start_broadcast().await;
        });
        write_log("[AppManager] start_udp_discovery: start_broadcast spawned");

        // 启动监听任务
        let udp_for_listener = udp.clone();
        tokio::spawn(async move {
            udp_for_listener.start_listener().await;
        });
        write_log("[AppManager] start_udp_discovery: start_listener spawned");

        // 保存主实例引用（用于 stop 和 get_hosts）
        self.udp_discovery = Some(udp);

        write_log("[AppManager] UDP discovery service started");
        Ok(())
    }

    /// 停止 UDP 发现服务
    pub fn stop_udp_discovery(&mut self) {
        if let Some(ref udp) = self.udp_discovery {
            udp.stop();
            write_log("[AppManager] UDP discovery service stopped");
        }
        self.udp_discovery = None;
    }

    /// 获取已发现的 Host 列表
    pub async fn get_discovered_hosts(&self) -> Vec<HostInfo> {
        if let Some(ref udp) = self.udp_discovery {
            udp.get_hosts().await
        } else {
            Vec::new()
        }
    }

    /// 发现局域网内的 Host（Client 模式）
    pub async fn discover_hosts(timeout_ms: u64) -> Vec<HostInfo> {
        UdpDiscoveryService::discover_hosts(timeout_ms).await
    }

    /// ============ HTTP 文件服务器 ============

    /// 添加共享目录
    pub async fn add_shared_dir(&self, name: String, path: String) -> Result<String, String> {
        if let Some(ref http) = self.http_server {
            Ok(http.add_shared_dir(name, PathBuf::from(path)).await)
        } else {
            Err("HTTP server not initialized".to_string())
        }
    }

    /// 移除共享目录
    pub async fn remove_shared_dir(&self, dir_id: String) -> Result<(), String> {
        if let Some(ref http) = self.http_server {
            http.remove_shared_dir(&dir_id).await;
            Ok(())
        } else {
            Err("HTTP server not initialized".to_string())
        }
    }

    /// 获取共享目录列表
    pub async fn get_shared_dirs(&self) -> Result<Vec<SharedDir>, String> {
        if let Some(ref http) = self.http_server {
            Ok(http.get_shared_dirs().await)
        } else {
            Err("HTTP server not initialized".to_string())
        }
    }

    /// 下载远程文件（流式下载，64KB 分片）
    pub async fn download_file(
        &self,
        host_ip: String,
        dir_id: String,
        file_path: String,
        save_dir: String,
    ) -> Result<String, String> {
        use tokio::io::AsyncWriteExt;
        use futures_util::StreamExt;

        // 如果未指定保存目录，使用系统下载目录下的 FeigeTransfer 子目录
        let save_dir = if save_dir.is_empty() {
            dirs::download_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("FeigeTransfer")
        } else {
            std::path::PathBuf::from(&save_dir)
        };

        // 确保目录存在
        std::fs::create_dir_all(&save_dir).map_err(|e| format!("创建目录失败: {}", e))?;

        let url = format!(
            "http://{}:18767/d/{}/{}",
            host_ip,
            dir_id,
            urlencoding::encode(&file_path)
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;

        let response = client.get(&url)
            .send()
            .await
            .map_err(|e| format!("连接失败: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("HTTP错误: {}", response.status()));
        }

        // 从 Content-Disposition 获取文件名
        let filename = {
            let cd = response
                .headers()
                .get("content-disposition")
                .and_then(|v| v.to_str().ok());

            if let Some(header_value) = cd {
                // 优先尝试 filename*=utf-8''... 格式
                if let Some(idx) = header_value.find("filename*=") {
                    let rest = &header_value[idx + 10..];
                    if let Some(stripped) = rest.strip_prefix("utf-8''") {
                        let name = stripped.trim_matches('"').trim_matches('\'');
                        if !name.is_empty() {
                            name.to_string()
                        } else {
                            extract_fallback_filename(header_value)
                        }
                    } else {
                        extract_fallback_filename(header_value)
                    }
                } else {
                    extract_fallback_filename(header_value)
                }
            } else {
                std::path::Path::new(&file_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "download".to_string())
            }
        };

        fn extract_fallback_filename(header_value: &str) -> String {
            if let Some(idx) = header_value.find("filename=") {
                let rest = &header_value[idx + 9..];
                let name = rest.trim_start_matches('"').trim_end_matches('"');
                if !name.is_empty() {
                    return name.to_string();
                }
            }
            "download".to_string()
        }

        // 路径遍历过滤：移除 ../ 和 ..\ 并取文件名部分
        let safe_filename = sanitize_path_component(&filename);
        let save_path = std::path::Path::new(&save_dir).join(&safe_filename);
        let total_size = response.content_length();

        write_log(&format!(
            "[Download] Starting: {} -> {:?}, size={:?}",
            url, save_path, total_size
        ));

        let mut file = tokio::fs::File::create(&save_path)
            .await
            .map_err(|e| format!("创建文件失败: {}", e))?;

        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = 0;

        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    // 下载出错，删除不完整的文件
                    let _ = tokio::fs::remove_file(&save_path).await;
                    return Err(format!("下载失败: {}", e));
                }
            };

            file.write_all(&chunk)
                .await
                .map_err(|e| format!("写入失败: {}", e))?;

            downloaded += chunk.len() as u64;

            // 可选：定期刷新
            if downloaded % (1024 * 1024) == 0 {
                file.flush().await.ok();
            }
        }

        file.flush()
            .await
            .map_err(|e| format!("刷新文件失败: {}", e))?;

        write_log(&format!(
            "[Download] Completed: {:?}, downloaded={}",
            save_path, downloaded
        ));

        Ok(save_path.to_string_lossy().to_string())
    }
}

// 全局应用管理器
static APP_MANAGER: std::sync::OnceLock<tokio::sync::Mutex<AppManager>> = std::sync::OnceLock::new();

fn get_manager() -> &'static tokio::sync::Mutex<AppManager> {
    APP_MANAGER.get_or_init(|| tokio::sync::Mutex::new(AppManager::new()))
}

// ============ Tauri Commands ============

#[tauri::command]
async fn init_app() -> Result<(), String> {
    let mut manager = get_manager().lock().await;
    manager.init().await
}

#[tauri::command]
async fn get_devices() -> Result<Vec<Device>, String> {
    let manager = get_manager().lock().await;
    Ok(manager.get_devices().await)
}

#[tauri::command]
async fn get_transfers() -> Result<Vec<TransferTask>, String> {
    let manager = get_manager().lock().await;
    Ok(manager.get_transfers().await)
}

#[tauri::command]
async fn get_device_name() -> Result<String, String> {
    let manager = get_manager().lock().await;
    Ok(manager.get_device_name().await)
}

#[tauri::command]
async fn set_device_name(name: String) -> Result<(), String> {
    let mut manager = get_manager().lock().await;
    manager.set_device_name(name).await;
    Ok(())
}

#[tauri::command]
async fn get_device_id() -> Result<String, String> {
    let manager = get_manager().lock().await;
    Ok(manager.get_device_id().await)
}

// 注意：不使用 rename_all，参数名与前端保持一致
#[tauri::command]
async fn send_file(targetIp: String, targetPort: u16, filePath: String) -> Result<String, String> {
    let manager = get_manager().lock().await;
    if let Some(ref transfer) = manager.transfer {
        transfer.send_file(targetIp, targetPort, PathBuf::from(filePath))
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Transfer service not initialized".to_string())
    }
}

#[tauri::command]
async fn select_file() -> Result<Option<String>, String> {
    Ok(None)
}

#[tauri::command]
async fn add_peer(peerIp: String, peerPort: u16) -> Result<Device, String> {
    let manager = get_manager().lock().await;
    manager.add_peer(peerIp, peerPort).await
}

#[tauri::command]
async fn scan_subnet(port: u16) -> Result<Vec<Device>, String> {
    let manager = get_manager().lock().await;
    Ok(manager.scan_subnet(port).await)
}

#[tauri::command]
async fn get_pending_requests() -> Result<Vec<crate::types::PendingFileRequest>, String> {
    let manager = get_manager().lock().await;
    Ok(manager.get_pending_requests().await)
}

#[tauri::command(rename_all = "snake_case")]
async fn confirm_receive(req_id: String, save_path: Option<String>) -> Result<(), String> {
    write_log(&format!("confirm_receive called with req_id={}, save_path={:?}", req_id, save_path));
    let manager = get_manager().lock().await;
    manager.confirm_receive(req_id, save_path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn reject_receive(req_id: String) -> Result<(), String> {
    write_log(&format!("reject_receive called with req_id={}", req_id));
    let manager = get_manager().lock().await;
    manager.reject_receive(req_id).await
}

#[tauri::command]
async fn get_transfer_progress() -> Result<Vec<crate::types::TransferTask>, String> {
    let manager = get_manager().lock().await;
    Ok(manager.get_transfers().await)
}

#[tauri::command]
async fn get_local_info() -> Result<(String, u16), String> {
    let manager = get_manager().lock().await;
    Ok((manager.get_local_ip(), manager.get_local_port()))
}

// ============ UDP 发现服务 Tauri Commands ============

#[tauri::command(rename_all = "snake_case")]
async fn start_discovery(name: String, port: u16, shared_dirs: Vec<SharedDir>) -> Result<(), String> {
    write_log(&format!("[Tauri] start_discovery called: name={}, port={}, dirs={}", name, port, shared_dirs.len()));
    let mut manager = get_manager().lock().await;
    manager.start_udp_discovery(name, port, shared_dirs).await
}

#[tauri::command(rename_all = "snake_case")]
async fn stop_discovery() -> Result<(), String> {
    write_log("[Tauri] stop_discovery called");
    let mut manager = get_manager().lock().await;
    manager.stop_udp_discovery();
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn discover_hosts(timeout_ms: u64) -> Result<Vec<HostInfo>, String> {
    write_log(&format!("[Tauri] discover_hosts called: timeout={}ms", timeout_ms));
    Ok(AppManager::discover_hosts(timeout_ms).await)
}

#[tauri::command(rename_all = "snake_case")]
async fn get_discovered_hosts() -> Result<Vec<HostInfo>, String> {
    let manager = get_manager().lock().await;
    Ok(manager.get_discovered_hosts().await)
}

// ============ HTTP 文件服务器 Tauri Commands ============

#[tauri::command(rename_all = "snake_case")]
async fn add_shared_dir(name: String, path: String) -> Result<String, String> {
    write_log(&format!("[Tauri] add_shared_dir: name={}, path={}", name, path));
    let manager = get_manager().lock().await;
    manager.add_shared_dir(name, path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn remove_shared_dir(dir_id: String) -> Result<(), String> {
    write_log(&format!("[Tauri] remove_shared_dir: dir_id={}", dir_id));
    let manager = get_manager().lock().await;
    manager.remove_shared_dir(dir_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn get_shared_dirs() -> Result<Vec<SharedDir>, String> {
    let manager = get_manager().lock().await;
    manager.get_shared_dirs().await
}

// 通过 HTTP 获取对方的共享目录列表（绕过浏览器 CORS 限制）
#[tauri::command]
async fn fetch_shared_dirs(hostIp: String) -> Result<HostInfo, String> {
    write_log(&format!("[Tauri] fetch_shared_dirs called: host={}", hostIp));
    let url = format!("http://{}:18767/", hostIp);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(&url)
        .send()
        .await
        .map_err(|e| format!("连接失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP错误: {}", resp.status()));
    }
    let host_info: HostInfo = resp.json::<HostInfo>().await.map_err(|e| format!("解析失败: {}", e))?;
    Ok(host_info)
}

// 通过 HTTP 浏览对方共享目录内容（绕过浏览器 CORS 限制）
#[tauri::command]
async fn browse_remote_dir(hostIp: String, dirId: String, dirPath: String) -> Result<Vec<crate::types::FileEntry>, String> {
    write_log(&format!("[Tauri] browse_remote_dir called: host={}, dir={}, path={}", hostIp, dirId, dirPath));
    let path_part = if dirPath.is_empty() {
        String::new()
    } else {
        format!("/{}", urlencoding::encode(&dirPath))
    };
    let url = format!("http://{}:18767/d/{}{}", hostIp, dirId, path_part);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(&url)
        .send()
        .await
        .map_err(|e| format!("连接失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP错误: {}", resp.status()));
    }
    let entries: Vec<crate::types::FileEntry> = resp.json::<Vec<crate::types::FileEntry>>()
        .await
        .map_err(|e| format!("解析失败: {}", e))?;
    Ok(entries)
}

// ============ 文件下载 Tauri Commands ============

// 注意：不使用 rename_all，参数名与前端保持一致（camelCase）
#[tauri::command]
async fn download_file(
    hostIp: String,
    dirId: String,
    filePath: String,
    saveDir: String,
) -> Result<String, String> {
    write_log(&format!(
        "[Tauri] download_file: host={}, dir={}, file={}, save={}",
        hostIp, dirId, filePath, saveDir
    ));
    let manager = get_manager().lock().await;
    manager.download_file(hostIp, dirId, filePath, saveDir).await
}

/// 递归统计目录下文件数量（同步递归，避免 async 递归问题）
fn count_files_in_dir_sync(host_ip: &str, dir_id: &str, dir_path: &str) -> Result<u32, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let entries: Vec<crate::types::FileEntry> = {
        let path_part = if dir_path.is_empty() {
            String::new()
        } else {
            format!("/{}", urlencoding::encode(dir_path))
        };
        let url = format!("http://{}:18767/d/{}{}", host_ip, dir_id, path_part);
        let resp = client.get(&url)
            .send()
            .map_err(|e| format!("连接失败: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP错误: {}", resp.status()));
        }
        resp.json::<Vec<crate::types::FileEntry>>()
            .map_err(|e| format!("解析失败: {}", e))?
    };

    let mut count = 0u32;
    for entry in entries {
        // 对 entry.name 做安全过滤，防止路径遍历
        let safe_name = sanitize_path_component(&entry.name);
        // 跳过无效名称
        if safe_name.is_empty() || safe_name == "download" {
            continue;
        }

        if entry.is_dir {
            let sub_path = if dir_path.is_empty() {
                safe_name
            } else {
                format!("{}/{}", dir_path, safe_name)
            };
            count += count_files_in_dir_sync(host_ip, dir_id, &sub_path)?;
        } else {
            count += 1;
        }
    }
    Ok(count)
}

/// 统计目录下文件数量（异步包装）
async fn count_files_in_dir_async(host_ip: &str, dir_id: &str, dir_path: &str) -> Result<u32, String> {
    // 使用 blocking client 在 thread pool 中执行同步递归
    let host_ip = host_ip.to_string();
    let dir_id = dir_id.to_string();
    let dir_path = dir_path.to_string();
    tokio::task::spawn_blocking(move || {
        count_files_in_dir_sync(&host_ip, &dir_id, &dir_path)
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))?
}

/// 递归下载目录（保持目录结构，同步版本避免 async 递归）
/// 返回 (downloaded_count, total_bytes)
fn download_dir_recursive_sync(
    host_ip: &str,
    dir_id: &str,
    remote_dir_path: &str,
    remote_dir_name: &str,
    save_base_dir: &str,
) -> Result<(u32, u64), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let entries: Vec<crate::types::FileEntry> = {
        let path_part = if remote_dir_path.is_empty() {
            String::new()
        } else {
            format!("/{}", urlencoding::encode(remote_dir_path))
        };
        let url = format!("http://{}:18767/d/{}{}", host_ip, dir_id, path_part);
        let resp = client.get(&url)
            .send()
            .map_err(|e| format!("连接失败: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP错误: {}", resp.status()));
        }
        resp.json::<Vec<crate::types::FileEntry>>()
            .map_err(|e| format!("解析失败: {}", e))?
    };

    // 本地保存路径：save_base_dir / remote_dir_name / (remote_dir_path 下的相对路径)
    let local_base = std::path::Path::new(save_base_dir).join(remote_dir_name);
    std::fs::create_dir_all(&local_base).map_err(|e| format!("创建目录失败: {}", e))?;

    let mut downloaded_count = 0u32;
    let mut total_bytes = 0u64;

    for entry in entries {
        let entry_rel_path = if remote_dir_path.is_empty() {
            entry.name.clone()
        } else {
            format!("{}/{}", remote_dir_path, entry.name)
        };

        // 对 entry.name 做安全过滤，防止路径遍历
        let safe_entry_name = sanitize_path_component(&entry.name);

        if entry.is_dir {
            let (sub_count, sub_bytes) = download_dir_recursive_sync(
                host_ip,
                dir_id,
                &entry_rel_path,
                &safe_entry_name,
                save_base_dir,
            )?;
            downloaded_count += sub_count;
            total_bytes += sub_bytes;
        } else {
            // 下载单个文件（blocking 方式）
            // 对相对路径做安全过滤，防止 .. 路径遍历
            let safe_rel_path = sanitize_rel_path(&entry_rel_path, &local_base)
                .map_err(|e| format!("路径不安全: {}", e))?;
            let local_file_path = local_base.join(&safe_rel_path);
            if let Some(parent) = local_file_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            let url = format!(
                "http://{}:18767/d/{}/{}",
                host_ip,
                dir_id,
                urlencoding::encode(&entry_rel_path)
            );
            let file_client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| e.to_string())?;
            let mut response = file_client.get(&url)
                .send()
                .map_err(|e| format!("下载失败: {}", e))?;
            if !response.status().is_success() {
                write_log(&format!("[download_dir] skipped {} (HTTP {})", entry_rel_path, response.status()));
                continue;
            }

            let mut file = std::fs::File::create(&local_file_path)
                .map_err(|e| format!("创建文件失败: {}", e))?;
            let mut file_size: u64 = 0;

            // 使用 blocking IO 复制到文件
            let mut writer = std::io::BufWriter::new(&file);
            std::io::copy(&mut response, &mut writer)
                .map_err(|e| format!("写入失败: {}", e))?;
            file_size = local_file_path.metadata().map(|m| m.len()).unwrap_or(0);

            downloaded_count += 1;
            total_bytes += file_size;
            write_log(&format!("[download_dir] {} ({} bytes)", entry_rel_path, file_size));
        }
    }

    Ok((downloaded_count, total_bytes))
}

/// 递归下载目录（异步包装）
async fn download_dir_recursive_async(
    host_ip: &str,
    dir_id: &str,
    remote_dir_path: &str,
    remote_dir_name: &str,
    save_base_dir: &str,
) -> Result<(u32, u64), String> {
    let host_ip = host_ip.to_string();
    let dir_id = dir_id.to_string();
    let remote_dir_path = remote_dir_path.to_string();
    let remote_dir_name = remote_dir_name.to_string();
    let save_base_dir = save_base_dir.to_string();
    tokio::task::spawn_blocking(move || {
        download_dir_recursive_sync(&host_ip, &dir_id, &remote_dir_path, &remote_dir_name, &save_base_dir)
    })
    .await
    .map_err(|e| format!("任务执行失败: {}", e))?
}

// 注意：不使用 rename_all，参数名与前端保持一致（camelCase）
#[tauri::command]
async fn download_dir(
    hostIp: String,
    dirId: String,
    dirPath: String,      // 远程目录的相对路径（相对于共享目录根）
    dirName: String,      // 远程目录名（用于创建本地子目录）
    saveDir: String,     // 本地保存目录
) -> Result<(u32, u64), String> {
    write_log(&format!(
        "[Tauri] download_dir: host={}, dir={}, path={}, name={}, save={}",
        hostIp, dirId, dirPath, dirName, saveDir
    ));

    let save_base = if saveDir.is_empty() {
        dirs::download_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("FeigeTransfer")
    } else {
        std::path::PathBuf::from(&saveDir)
    };

    let (count, bytes) = download_dir_recursive_async(&hostIp, &dirId, &dirPath, &dirName, save_base.to_str().unwrap_or(".")).await?;

    write_log(&format!("[Tauri] download_dir completed: {} files, {} bytes", count, bytes));
    Ok((count, bytes))
}

/// 统计目录下文件数量（供前端大目录确认用）
#[tauri::command]
async fn count_remote_dir_files(hostIp: String, dirId: String, dirPath: String) -> Result<u32, String> {
    write_log(&format!(
        "[Tauri] count_remote_dir_files: host={}, dir={}, path={}",
        hostIp, dirId, dirPath
    ));
    count_files_in_dir_async(&hostIp, &dirId, &dirPath).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|_app| {
            // 在单独的线程中运行 tokio runtime
            std::thread::spawn(|| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    // 先初始化
                    let init_result = {
                        let mut manager = get_manager().lock().await;
                        manager.init().await
                    };
                    
                    match init_result {
                        Ok(_) => {
                            write_log("[FeigeTransfer] Services started");
                        },
                        Err(e) => {
                            write_log(&format!("[FeigeTransfer] Init error: {}", e));
                        },
                    }
                    
                    // 初始化完成后，manager 的 lock 就被释放了
                    // 然后进入一个无限循环保持 runtime 活跃
                    // 但不使用 manager lock
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                    }
                });
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init_app,
            get_devices,
            get_transfers,
            get_device_name,
            set_device_name,
            get_device_id,
            send_file,
            select_file,
            add_peer,
            scan_subnet,
            get_pending_requests,
            confirm_receive,
            reject_receive,
            get_local_info,
            get_transfer_progress,
            start_discovery,
            stop_discovery,
            discover_hosts,
            get_discovered_hosts,
            add_shared_dir,
            remove_shared_dir,
            get_shared_dirs,
            fetch_shared_dirs,
            browse_remote_dir,
            download_file,
            download_dir,
            count_remote_dir_files
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
