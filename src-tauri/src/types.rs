use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// ============ 共享目录相关类型 ============

/// 共享目录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedDir {
    pub id: String,       // 目录唯一ID
    pub name: String,    // 显示名称
    pub path: String,    // 实际路径（序列化用 String）
}

/// 主机信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub shared_dirs: Vec<SharedDir>,
}

/// UDP 发现消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryMessage {
    #[serde(rename = "type")]
    pub msg_type: String,         // "announce" 或 "who-is-host"
    pub id: String,                 // 设备ID
    pub name: String,               // 设备名称
    pub port: u16,                  // TCP 文件传输端口
    #[serde(default)]
    pub shared_dirs: Vec<SharedDir>, // 共享目录（announce 消息携带）
}

/// 文件条目（用于目录浏览）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,     // 文件/目录名称
    pub path: String,      // 相对于共享目录根的路径
    pub is_dir: bool,     // 是否是目录
    pub size: u64,         // 文件大小（目录时为0）
    pub modified: Option<u64>, // 修改时间戳（秒）
}

/// ============ 原有类型 ============

/// 设备信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,           // 设备唯一ID
    pub name: String,         // 设备名称
    pub ip: String,           // IP地址
    pub port: u16,            // TCP端口
    pub online: bool,         // 在线状态
    pub last_seen: i64,       // 最后活跃时间戳
}

/// 文件传输任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferTask {
    pub id: String,           // 传输ID
    pub file_name: String,    // 文件名
    pub file_size: u64,       // 文件大小
    pub transferred: u64,     // 已传输大小
    pub progress: f32,        // 进度 (0-100)
    pub speed: u64,          // 传输速度 (bytes/s)
    pub direction: TransferDirection,
    pub status: TransferStatus,
    pub peer_id: String,      // 对端设备ID
    pub peer_name: String,    // 对端设备名称
}

/// 待接收文件请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingFileRequest {
    pub id: String,           // 请求ID
    pub file_name: String,    // 文件名
    pub file_size: u64,       // 文件大小
    pub sender_ip: String,    // 发送方IP
    pub is_folder: bool,     // 是否是文件夹
    pub file_count: u32,      // 文件数量（文件夹时有效）
    pub total_size: u64,      // 总大小（文件夹时有效）
}

/// 文件夹传输元信息
#[derive(Debug, Clone)]
pub struct FolderTransferMeta {
    pub folder_name: String,
    pub total_files: u32,
    pub total_size: u64,
    pub files: Vec<FolderFileInfo>,
}

/// 文件夹内的文件信息
#[derive(Debug, Clone)]
pub struct FolderFileInfo {
    pub relative_path: String,  // 相对于文件夹的路径
    pub size: u64,               // 文件大小
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransferDirection {
    Send,   // 发送
    Receive,// 接收
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransferStatus {
    Pending,    // 待确认
    Transferring, // 传输中
    Completed,   // 完成
    Failed,      // 失败
    Cancelled,   // 取消
}

/// 传输请求 (接收方收到的请求)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    pub task_id: String,
    pub file_name: String,
    pub file_size: u64,
    pub sender_id: String,
    pub sender_name: String,
}

/// 应用状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub device_id: String,
    pub device_name: String,
    pub devices: Vec<Device>,
    pub transfers: Vec<TransferTask>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: whoami::fallible::hostname().unwrap_or_else(|_| "我的电脑".to_string()),
            devices: Vec::new(),
            transfers: Vec::new(),
        }
    }
}
