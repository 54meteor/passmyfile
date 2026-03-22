use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::fs::OpenOptions;
use std::io::Write;

mod types;
mod discovery;
mod transfer;

use types::{Device, TransferTask, AppState};
use discovery::DiscoveryService;
use transfer::TransferService;

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
}

impl AppManager {
    pub fn new() -> Self {
        let state = AppState::default();
        
        Self {
            state: Arc::new(RwLock::new(state)),
            discovery: None,
            transfer: None,
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
            downloads_dir,
        );
        
        // 启动服务
        discovery.start().await;
        transfer.start().await.map_err(|e| e.to_string())?;
        
        self.discovery = Some(discovery);
        self.transfer = Some(transfer);
        
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

#[tauri::command]
async fn send_file(target_ip: String, target_port: u16, file_path: String) -> Result<String, String> {
    let manager = get_manager().lock().await;
    if let Some(ref transfer) = manager.transfer {
        transfer.send_file(target_ip, target_port, PathBuf::from(file_path))
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
async fn add_peer(peer_ip: String, peer_port: u16) -> Result<Device, String> {
    let manager = get_manager().lock().await;
    manager.add_peer(peer_ip, peer_port).await
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
            get_transfer_progress
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
