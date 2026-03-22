use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, RwLock, mpsc};
use chrono::Utc;

use crate::write_log;

use crate::types::Device;

const TRANSFER_PORT: u16 = 18766;

/// 设备管理服务
pub struct DiscoveryService {
    device_id: String,
    device_name: String,
    tcp_port: u16,
    local_ip: Option<String>,
    devices: Arc<RwLock<HashMap<String, Device>>>,
    tx: broadcast::Sender<Device>,
}

impl DiscoveryService {
    /// 创建新的发现服务
    pub fn new(device_id: String, device_name: String, tcp_port: u16) -> Result<Self> {
        let local_ip = Self::get_local_ip();
        write_log(&format!("Local IP: {:?}", local_ip));
        
        let (tx, _) = broadcast::channel(100);
        
        Ok(Self {
            device_id,
            device_name,
            tcp_port,
            local_ip,
            devices: Arc::new(RwLock::new(HashMap::new())),
            tx,
        })
    }

    /// 获取本地IP地址
    pub fn get_local_ip() -> Option<String> {
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    return Some(addr.ip().to_string());
                }
            }
        }
        None
    }

    /// 启动服务（现在只是记录日志）
    pub async fn start(&self) {
        write_log("DiscoveryService started");
        write_log(&format!("Device ID: {}", self.device_id));
        write_log(&format!("Device Name: {}", self.device_name));
        write_log(&format!("Local IP: {:?}", self.local_ip));
        write_log(&format!("TCP Port: {}", self.tcp_port));
    }

    /// 手动添加主机 - 尝试 TCP 连接对方
    pub async fn add_peer(&self, peer_ip: String, peer_port: u16) -> Result<Device> {
        let addr = format!("{}:{}", peer_ip, peer_port);
        write_log(&format!("Attempting to connect to peer: {}", addr));
        
        // 尝试 TCP 连接
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(3),
            TcpStream::connect(&addr)
        ).await {
            Ok(Ok(_stream)) => {
                // 连接成功，添加到设备列表
                let device = Device {
                    id: format!("{}-{}", peer_ip, peer_port),
                    name: format!("主机 ({})", peer_ip),
                    ip: peer_ip.clone(),
                    port: peer_port,
                    online: true,
                    last_seen: Utc::now().timestamp(),
                };
                
                {
                    let mut devs = self.devices.write().await;
                    devs.insert(device.id.clone(), device.clone());
                }
                
                let _ = self.tx.send(device.clone());
                write_log(&format!("Peer added successfully: {}", peer_ip));
                Ok(device)
            }
            Ok(Err(e)) => {
                write_log(&format!("Failed to connect to {}: {}", addr, e));
                Err(anyhow::anyhow!("连接失败: {}", e))
            }
            Err(_) => {
                write_log(&format!("Connection to {} timed out", addr));
                Err(anyhow::anyhow!("连接超时"))
            }
        }
    }

    /// 扫描子网发现主机
    pub async fn scan_subnet(&self, port: u16) -> Vec<Device> {
        let local_ip = self.local_ip.clone();
        if local_ip.is_none() {
            write_log("No local IP, cannot scan");
            return Vec::new();
        }
        
        let ip = local_ip.unwrap();
        let parts: Vec<&str> = ip.split('.').collect();
        if parts.len() < 3 {
            write_log("Invalid local IP format");
            return Vec::new();
        }
        
        let subnet = format!("{}.{}.{}", parts[0], parts[1], parts[2]);
        let local_full_ip = ip.clone(); // 保存本机完整IP用于过滤
        write_log(&format!("Starting subnet scan: {}.x (excluding {})", subnet, local_full_ip));
        
        let mut found_devices = Vec::new();
        
        // 并发扫描，使用通道限制并发数
        let (tx, mut rx) = mpsc::channel(50);
        let mut handles = Vec::new();
        
        for i in 1..=254 {
            let tx = tx.clone();
            let subnet = subnet.clone();
            let port = port;
            
            let handle = tokio::spawn(async move {
                let target_ip = format!("{}.{}", subnet, i);
                let addr = format!("{}:{}", target_ip, port);
                
                // 尝试 TCP 连接，超时 500ms
                match tokio::time::timeout(
                    tokio::time::Duration::from_millis(500),
                    TcpStream::connect(&addr)
                ).await {
                    Ok(Ok(_stream)) => {
                        let _ = tx.send((target_ip, port)).await;
                    }
                    _ => {}
                }
            });
            handles.push(handle);
        }
        
        // 等待所有任务完成
        drop(tx);
        while let Some((ip, port)) = rx.recv().await {
            // 排除本机IP
            if ip == local_full_ip {
                write_log(&format!("Skipping self: {}", ip));
                continue;
            }
            
            let device = Device {
                id: format!("{}-{}", ip, port),
                name: format!("主机 ({})", ip),
                ip: ip.clone(),
                port,
                online: true,
                last_seen: Utc::now().timestamp(),
            };
            found_devices.push(device);
            write_log(&format!("Found device: {}", ip));
        }
        
        // 等待所有任务完成
        for handle in handles {
            let _ = handle.await;
        }
        
        // 添加到设备列表
        {
            let mut devs = self.devices.write().await;
            for device in &found_devices {
                devs.insert(device.id.clone(), device.clone());
            }
        }
        
        write_log(&format!("Subnet scan completed, found {} devices", found_devices.len()));
        found_devices
    }

    /// 获取当前设备列表
    pub async fn get_devices(&self) -> Vec<Device> {
        let devs = self.devices.read().await;
        devs.values().cloned().collect()
    }

    /// 获取本机 IP
    pub fn get_local_ip_str(&self) -> String {
        self.local_ip.clone().unwrap_or_else(|| "未知".to_string())
    }

    /// 获取本机端口
    pub fn get_local_port(&self) -> u16 {
        self.tcp_port
    }

    /// 获取设备列表变更的订阅
    pub fn subscribe(&self) -> broadcast::Receiver<Device> {
        self.tx.subscribe()
    }
}
