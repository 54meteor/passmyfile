import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import './App.css';

// 类型定义
interface Device {
  id: string;
  name: string;
  ip: string;
  port: number;
  online: boolean;
  last_seen: number;
}

interface TransferTask {
  id: string;
  file_name: string;
  file_size: number;
  transferred: number;
  progress: number;
  speed: number;
  direction: 'Send' | 'Receive';
  status: string;
  peer_id: string;
  peer_name: string;
}

interface PendingRequest {
  id: string;
  file_name: string;
  file_size: number;
  sender_ip: string;
  is_folder: boolean;
  file_count: number;
  total_size: number;
}

// 格式化文件大小
function formatSize(bytes: number): string {
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
  if (bytes < 1024 * 1024 * 1024) return (bytes / 1024 / 1024).toFixed(1) + ' MB';
  return (bytes / 1024 / 1024 / 1024).toFixed(2) + ' GB';
}

function App() {
  const [deviceName, setDeviceName] = useState('');
  const [localIp, setLocalIp] = useState('');
  const [localPort, setLocalPort] = useState(18766);
  const [devices, setDevices] = useState<Device[]>([]);
  const [transfers, setTransfers] = useState<TransferTask[]>([]);
  const [selectedDevice, setSelectedDevice] = useState<Device | null>(null);
  
  // 输入框状态
  const [peerIp, setPeerIp] = useState('');
  const [peerPort, setPeerPort] = useState('18766');
  const [isAdding, setIsAdding] = useState(false);
  const [isScanning, setIsScanning] = useState(false);
  
  // 待接收文件状态
  const [pendingRequests, setPendingRequests] = useState<PendingRequest[]>([]);
  const [showReceiveModal, setShowReceiveModal] = useState(false);

  // 初始化应用
  useEffect(() => {
    const init = async () => {
      try {
        // 等待服务初始化
        await new Promise(resolve => setTimeout(resolve, 500));
        
        try {
          const name = await invoke<string>('get_device_name');
          const [ip, port] = await invoke<[string, number]>('get_local_info');
          
          setDeviceName(name);
          setLocalIp(ip);
          setLocalPort(port);
        } catch (e) {
          console.error('Failed to get local info:', e);
          setLocalIp('初始化中...');
        }
        
        // 定时刷新设备列表
        const deviceInterval = setInterval(async () => {
          try {
            const devs = await invoke<Device[]>('get_devices');
            setDevices(devs);
          } catch (e) {
            // 忽略错误
          }
        }, 2000);
        
        // 定时检查待接收文件
        const requestInterval = setInterval(async () => {
          try {
            const pending = await invoke<PendingRequest[]>('get_pending_requests');
            console.log('Pending requests:', pending.length);
            if (pending.length > 0) {
              console.log('Showing modal for:', pending[0].file_name);
              setPendingRequests(pending);
              setShowReceiveModal(true);
            } else {
              setShowReceiveModal(false);
            }
          } catch (e) {
            console.error('Failed to get pending requests:', e);
          }
        }, 1000);
        
        // 定时刷新传输进度
        const transferInterval = setInterval(async () => {
          try {
            const progress = await invoke<TransferTask[]>('get_transfer_progress');
            setTransfers(progress);
          } catch (e) {
            // 忽略错误
          }
        }, 500);
        
        return () => {
          clearInterval(deviceInterval);
          clearInterval(requestInterval);
          clearInterval(transferInterval);
        };
      } catch (e) {
        console.error('Init failed:', e);
      }
    };
    
    init();
  }, []);

  // 更新设备名称
  const handleNameChange = async (newName: string) => {
    try {
      await invoke('set_device_name', { name: newName });
      setDeviceName(newName);
    } catch (e) {
      console.error('Failed to set name:', e);
    }
  };

  // 添加主机
  const handleAddPeer = async () => {
    if (!peerIp.trim()) return;
    
    setIsAdding(true);
    try {
      const port = parseInt(peerPort) || 18766;
      const device = await invoke<Device>('add_peer', { 
        peerIp: peerIp.trim(), 
        peerPort: port 
      });
      
      // 添加到列表
      setDevices(prev => [...prev.filter(d => d.id !== device.id), device]);
      setSelectedDevice(device);
      setPeerIp('');
    } catch (e) {
      alert('连接失败：' + e);
    } finally {
      setIsAdding(false);
    }
  };

  // 扫描子网
  const handleScan = async () => {
    setIsScanning(true);
    try {
      const found = await invoke<Device[]>('scan_subnet', { port: 18766 });
      
      if (found.length === 0) {
        alert('未发现任何设备');
      } else {
        // 添加到列表
        setDevices(prev => {
          const newList = [...prev];
          for (const device of found) {
            if (!newList.some(d => d.id === device.id)) {
              newList.push(device);
            }
          }
          return newList;
        });
        alert(`发现 ${found.length} 台设备！`);
      }
    } catch (e) {
      alert('扫描失败：' + e);
    } finally {
      setIsScanning(false);
    }
  };

  // 发送文件
  const handleSendFile = async () => {
    if (!selectedDevice) return;
    
    try {
      // 使用 Tauri 文件对话框选择文件
      const filePath = await open({
        multiple: false,
        directory: false,
        title: '选择要发送的文件'
      });
      
      if (!filePath) return; // 用户取消了
      
      await invoke('send_file', {
        targetIp: selectedDevice.ip,
        targetPort: selectedDevice.port,
        filePath: filePath
      });
      alert(`文件已发送给 ${selectedDevice.name}`);
    } catch (e) {
      console.error('Send failed:', e);
      alert('发送失败：' + e);
    }
  };

  // 发送文件夹
  const handleSendFolder = async () => {
    if (!selectedDevice) return;
    
    try {
      // 使用 Tauri 文件对话框选择文件夹
      const folderPath = await open({
        directory: true,
        title: '选择要发送的文件夹'
      });
      
      if (!folderPath) return; // 用户取消了
      
      await invoke('send_file', {
        targetIp: selectedDevice.ip,
        targetPort: selectedDevice.port,
        filePath: folderPath
      });
      alert(`文件夹已发送给 ${selectedDevice.name}`);
    } catch (e) {
      console.error('Send failed:', e);
      alert('发送失败：' + e);
    }
  };

  // 接受文件（使用默认路径）
  const handleAcceptFile = async (reqId: string) => {
    try {
      console.log('Accepting file with ID:', reqId);
      await invoke('confirm_receive', { req_id: reqId, save_path: null });
      console.log('Confirm success');
      setShowReceiveModal(false);
      setPendingRequests([]);
      alert('文件已接收');
    } catch (e) {
      console.error('Accept failed:', e);
      alert('接收失败：' + String(e));
    }
  };

  // 接受文件并选择保存路径
  const handleAcceptFileAs = async (reqId: string) => {
    try {
      const path = await open({
        directory: true,
        title: '选择保存位置'
      });
      if (!path) return;
      
      console.log('Accepting file with ID:', reqId, 'path:', path);
      await invoke('confirm_receive', { req_id: reqId, save_path: path });
      console.log('Confirm success');
      setShowReceiveModal(false);
      setPendingRequests([]);
      alert('文件已接收');
    } catch (e) {
      console.error('Accept failed:', e);
      alert('接收失败：' + String(e));
    }
  };

  // 拒绝文件
  const handleRejectFile = async (reqId: string) => {
    try {
      console.log('Rejecting file with ID:', reqId);
      await invoke('reject_receive', { req_id: reqId });
      console.log('Reject success');
      setShowReceiveModal(false);
      setPendingRequests([]);
    } catch (e) {
      console.error('Reject failed:', e);
      alert('操作失败：' + String(e));
    }
  };

  return (
    <div className="app">
      <header className="header">
        <h1>📡 隔空投送</h1>
        <div className="device-info">
          <span>本机IP: <strong>{localIp}:{localPort}</strong></span>
          <input
            type="text"
            value={deviceName}
            onChange={(e) => setDeviceName(e.target.value)}
            onBlur={(e) => handleNameChange(e.target.value)}
            placeholder="输入设备名称"
            className="name-input"
          />
        </div>
      </header>

      <main className="main">
        <section className="devices-section">
          <h2>在线设备</h2>
          
          {/* 添加主机表单 */}
          <div className="add-peer-form">
            <h3>添加主机</h3>
            <div className="form-row">
              <input
                type="text"
                value={peerIp}
                onChange={(e) => setPeerIp(e.target.value)}
                placeholder="对方IP，如 192.168.1.100"
                className="peer-input"
              />
              <input
                type="text"
                value={peerPort}
                onChange={(e) => setPeerPort(e.target.value)}
                placeholder="端口"
                className="port-input"
              />
              <button 
                className="add-btn"
                onClick={handleAddPeer}
                disabled={isAdding || !peerIp.trim()}
              >
                {isAdding ? '连接中...' : '添加'}
              </button>
            </div>
            
            <div className="form-row" style={{ marginTop: '12px' }}>
              <button 
                className="scan-btn"
                onClick={handleScan}
                disabled={isScanning}
              >
                {isScanning ? '🔍 扫描中...' : '🔍 扫描局域网'}
              </button>
            </div>
          </div>
          
          <div className="device-list">
            {devices.length === 0 ? (
              <div className="empty-state">
                <p>未发现其他设备</p>
                <p className="hint">在上方输入对方IP并点击"添加"来连接</p>
              </div>
            ) : (
              devices.map((device) => (
                <div
                  key={device.id}
                  className={`device-card ${selectedDevice?.id === device.id ? 'selected' : ''}`}
                  onClick={() => setSelectedDevice(device)}
                >
                  <div className="device-icon">💻</div>
                  <div className="device-details">
                    <div className="device-name">{device.name}</div>
                    <div className="device-ip">{device.ip}:{device.port}</div>
                  </div>
                  <div className="device-status online">在线</div>
                </div>
              ))
            )}
          </div>
        </section>

        <section className="transfer-section">
          <h2>文件传输</h2>
          
          {selectedDevice ? (
            <div className="send-panel">
              <p>选择文件发送给: <strong>{selectedDevice.name}</strong></p>
              <p className="peer-info">IP: {selectedDevice.ip}:{selectedDevice.port}</p>
              <button 
                className="send-btn"
                onClick={handleSendFile}
              >
                📄 选择文件发送
              </button>
              <button 
                className="send-btn send-folder-btn"
                onClick={handleSendFolder}
              >
                📂 选择文件夹发送
              </button>
            </div>
          ) : (
            <div className="empty-state">
              <p>请先添加并选择接收设备</p>
            </div>
          )}

          <div className="transfer-list">
            <h3>传输记录</h3>
            {transfers.length === 0 ? (
              <p className="hint">暂无传输记录</p>
            ) : (
              transfers.map((t) => (
                <div key={t.id} className="transfer-item">
                  <div className="transfer-info">
                    <span className="transfer-name">{t.file_name}</span>
                    <span className="transfer-status">{t.direction === 'Send' ? '→' : '←'} {t.status}</span>
                  </div>
                  <div className="progress-bar">
                    <div className="progress-fill" style={{ width: `${t.progress}%` }}></div>
                  </div>
                  <div className="transfer-meta">
                    <span>{formatSize(t.transferred)} / {formatSize(t.file_size)}</span>
                    <span>{t.progress.toFixed(1)}%</span>
                  </div>
                </div>
              ))
            )}
          </div>
        </section>
      </main>

      {/* 文件接收确认弹窗 */}
      {showReceiveModal && pendingRequests.length > 0 && (
        <div className="modal-overlay">
          <div className="modal-content">
            <h2>📥 收到{pendingRequests[0].is_folder ? '文件夹' : '文件'}</h2>
            <div className="file-info">
              <p><strong>名称：</strong>{pendingRequests[0].file_name}</p>
              {pendingRequests[0].is_folder ? (
                <>
                  <p><strong>文件数：</strong>{pendingRequests[0].file_count} 个文件</p>
                  <p><strong>总大小：</strong>{formatSize(pendingRequests[0].total_size)}</p>
                </>
              ) : (
                <p><strong>大小：</strong>{formatSize(pendingRequests[0].file_size)}</p>
              )}
              <p><strong>来自：</strong>{pendingRequests[0].sender_ip}</p>
            </div>
            <div className="modal-buttons">
              <button 
                className="modal-btn accept-btn"
                onClick={() => handleAcceptFile(pendingRequests[0].id)}
              >
                📥 接收（默认路径）
              </button>
              <button 
                className="modal-btn accept-as-btn"
                onClick={() => handleAcceptFileAs(pendingRequests[0].id)}
              >
                📂 另存为...
              </button>
              <button 
                className="modal-btn reject-btn"
                onClick={() => handleRejectFile(pendingRequests[0].id)}
              >
                ❌ 拒绝
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
