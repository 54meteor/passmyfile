use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, RwLock};
use tokio::time::{interval, Duration};

use crate::types::{DiscoveryMessage, HostInfo, SharedDir};
use crate::write_log;

const UDP_DISCOVERY_PORT: u16 = 18768;
const BROADCAST_ADDR: &str = "255.255.255.255:18768";
const ANNOUNCE_INTERVAL_SECS: u64 = 5;
const DISCOVER_TIMEOUT_SECS: u64 = 3;

/// UDP 发现服务（Host 广播 + Client 扫描）
#[derive(Clone)]
pub struct UdpDiscoveryService {
    device_id: String,
    device_name: String,
    tcp_port: u16,
    shared_dirs: Arc<RwLock<Vec<SharedDir>>>,
    hosts: Arc<RwLock<HashMap<String, HostInfo>>>,
    shutdown_tx: broadcast::Sender<()>,
}

impl UdpDiscoveryService {
    /// 创建新的 UDP 发现服务
    pub fn new(device_id: String, device_name: String, tcp_port: u16) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        Self {
            device_id,
            device_name,
            tcp_port,
            shared_dirs: Arc::new(RwLock::new(Vec::new())),
            hosts: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx,
        }
    }

    /// 动态更新共享目录列表
    pub async fn update_shared_dirs(&self, dirs: Vec<SharedDir>) {
        let mut sd = self.shared_dirs.write().await;
        *sd = dirs;
    }

    /// 启动 Host 广播（每 5 秒广播一次）
    pub async fn start_broadcast(&self) {
        let socket = match UdpSocket::bind(format!("0.0.0.0:{}", UDP_DISCOVERY_PORT)).await {
            Ok(s) => s,
            Err(e) => {
                write_log(&format!("[UdpDiscovery] Failed to bind UDP socket: {}", e));
                return;
            }
        };

        // 设置广播权限
        if let Err(e) = socket.set_broadcast(true) {
            write_log(&format!("[UdpDiscovery] Failed to set broadcast: {}", e));
            return;
        }

        write_log(&format!("[UdpDiscovery] Started broadcasting on port {}", UDP_DISCOVERY_PORT));

        let mut ticker = interval(Duration::from_secs(ANNOUNCE_INTERVAL_SECS));
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let shared_dirs = self.shared_dirs.clone();

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // 每次广播时从 lock 读取最新的 shared_dirs
                    let dirs = shared_dirs.read().await.clone();
                    let announce_msg = DiscoveryMessage {
                        msg_type: "announce".to_string(),
                        id: self.device_id.clone(),
                        name: self.device_name.clone(),
                        port: self.tcp_port,
                        shared_dirs: dirs,
                    };
                    if let Ok(msg_bytes) = serde_json::to_vec(&announce_msg) {
                        if let Err(e) = socket.send_to(&msg_bytes, BROADCAST_ADDR).await {
                            write_log(&format!("[UdpDiscovery] Broadcast error: {}", e));
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    write_log("[UdpDiscovery] Broadcast stopped");
                    break;
                }
            }
        }
    }

    /// 启动监听（接收 announce 和 who-is-host 消息）
    pub async fn start_listener(&self) {
        let socket = match UdpSocket::bind(format!("0.0.0.0:{}", UDP_DISCOVERY_PORT)).await {
            Ok(s) => s,
            Err(e) => {
                write_log(&format!("[UdpDiscovery] Failed to bind UDP listener: {}", e));
                return;
            }
        };

        // 设置广播权限
        if let Err(e) = socket.set_broadcast(true) {
            write_log(&format!("[UdpDiscovery] Failed to set broadcast on listener: {}", e));
            return;
        }

        let hosts = self.hosts.clone();
        let device_id = self.device_id.clone();
        let tcp_port = self.tcp_port;
        let shared_dirs = self.shared_dirs.clone(); // Arc<RwLock<Vec<SharedDir>>>
        let device_name = self.device_name.clone();

        write_log(&format!("[UdpDiscovery] Started listening on port {}", UDP_DISCOVERY_PORT));

        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut buf = [0u8; 4096];

        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, addr)) => {
                            if let Err(e) = Self::handle_message(
                                &buf[..len],
                                addr,
                                &hosts,
                                &device_id,
                                &device_name,
                                tcp_port,
                                &shared_dirs,
                                &socket,
                            ).await {
                                write_log(&format!("[UdpDiscovery] Handle message error: {}", e));
                            }
                        }
                        Err(e) => {
                            write_log(&format!("[UdpDiscovery] Recv error: {}", e));
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    write_log("[UdpDiscovery] Listener stopped");
                    break;
                }
            }
        }
    }

    /// 处理收到的 UDP 消息
    async fn handle_message(
        buf: &[u8],
        addr: SocketAddr,
        hosts: &Arc<RwLock<HashMap<String, HostInfo>>>,
        self_id: &str,
        self_name: &str,
        self_port: u16,
        self_shared_dirs: &Arc<RwLock<Vec<SharedDir>>>,
        socket: &UdpSocket,
    ) -> Result<()> {
        let msg: DiscoveryMessage = serde_json::from_slice(buf)?;

        // 忽略自己发送的消息
        if msg.id == self_id {
            return Ok(());
        }

        match msg.msg_type.as_str() {
            "announce" => {
                write_log(&format!("[UdpDiscovery] Received announce from {} ({})", msg.name, addr));
                
                let host_info = HostInfo {
                    id: msg.id.clone(),
                    name: msg.name.clone(),
                    ip: addr.ip().to_string(),
                    port: msg.port,
                    shared_dirs: msg.shared_dirs.clone(),
                };

                let mut h = hosts.write().await;
                h.insert(msg.id.clone(), host_info);
            }
            "who-is-host" => {
                write_log(&format!("[UdpDiscovery] Received who-is-host from {}", addr));
                
                // 立即回复 announce（从 lock 读取最新的 shared_dirs）
                let dirs = self_shared_dirs.read().await.clone();
                let response = DiscoveryMessage {
                    msg_type: "announce".to_string(),
                    id: self_id.to_string(),
                    name: self_name.to_string(),
                    port: self_port,
                    shared_dirs: dirs,
                };

                if let Ok(resp_bytes) = serde_json::to_vec(&response) {
                    if let Err(e) = socket.send_to(&resp_bytes, addr).await {
                        write_log(&format!("[UdpDiscovery] Failed to send response: {}", e));
                    }
                }
            }
            _ => {
                write_log(&format!("[UdpDiscovery] Unknown message type: {}", msg.msg_type));
            }
        }

        Ok(())
    }

    /// 停止广播和监听
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(());
        write_log("[UdpDiscovery] Stop signal sent");
    }

    /// Client: 发送 who-is-host 并等待回复（超时 3 秒）
    pub async fn discover_hosts(timeout_ms: u64) -> Vec<HostInfo> {
        let socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(e) => {
                write_log(&format!("[UdpDiscovery] Client failed to bind: {}", e));
                return Vec::new();
            }
        };

        if let Err(e) = socket.set_broadcast(true) {
            write_log(&format!("[UdpDiscovery] Client failed to set broadcast: {}", e));
            return Vec::new();
        }

        // 发送 who-is-host
        let who_is_host = DiscoveryMessage {
            msg_type: "who-is-host".to_string(),
            id: String::new(),
            name: String::new(),
            port: 0,
            shared_dirs: Vec::new(),
        };

        let msg_bytes = match serde_json::to_vec(&who_is_host) {
            Ok(b) => b,
            Err(e) => {
                write_log(&format!("[UdpDiscovery] Failed to serialize who-is-host: {}", e));
                return Vec::new();
            }
        };

        if let Err(e) = socket.send_to(&msg_bytes, BROADCAST_ADDR).await {
            write_log(&format!("[UdpDiscovery] Failed to send who-is-host: {}", e));
            return Vec::new();
        }

        write_log(&format!("[UdpDiscovery] Sent who-is-host, waiting {}ms for responses...", timeout_ms));

        let mut hosts: HashMap<String, HostInfo> = HashMap::new();
        let mut buf = [0u8; 4096];
        let timeout = Duration::from_millis(timeout_ms);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, addr)) => {
                            if let Ok(msg) = serde_json::from_slice::<DiscoveryMessage>(&buf[..len]) {
                                if msg.msg_type == "announce" {
                                    let ip = addr.ip().to_string();
                                    write_log(&format!("[UdpDiscovery] Discovered host: {} ({}) at {}", msg.name, msg.id, ip));
                                    hosts.insert(msg.id.clone(), HostInfo {
                                        id: msg.id,
                                        name: msg.name,
                                        ip,
                                        port: msg.port,
                                        shared_dirs: msg.shared_dirs,
                                    });
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(remaining) => {
                    break;
                }
            }
        }

        write_log(&format!("[UdpDiscovery] Discovery complete, found {} hosts", hosts.len()));
        hosts.into_values().collect()
    }

    /// 获取已发现的 Host 列表
    pub async fn get_hosts(&self) -> Vec<HostInfo> {
        let h = self.hosts.read().await;
        h.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_message_serde() {
        let msg = DiscoveryMessage {
            msg_type: "announce".to_string(),
            id: "test-id".to_string(),
            name: "Test Host".to_string(),
            port: 18766,
            shared_dirs: vec![SharedDir {
                id: "dir1".to_string(),
                name: "Downloads".to_string(),
                path: "C:\\Downloads".to_string(),
            }],
        };

        let json = serde_json::to_string(&msg).unwrap();
        println!("{}", json);
        
        let parsed: DiscoveryMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.msg_type, "announce");
        assert_eq!(parsed.id, "test-id");
        assert_eq!(parsed.name, "Test Host");
        assert_eq!(parsed.port, 18766);
        assert_eq!(parsed.shared_dirs.len(), 1);
    }
}
