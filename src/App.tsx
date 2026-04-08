import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import './App.css';

// ============ Types ============
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

interface SharedDir {
  id: string;
  name: string;
  path: string;
}

interface HostInfo {
  id: string;
  name: string;
  ip: string;
  port: number;
  shared_dirs: SharedDir[];
}

interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified: number | null;
}

interface DownloadProgress {
  id: string;
  file_name: string;
  total: number;
  downloaded: number;
  progress: number;
  is_dir?: boolean;
  file_count?: number;
}

// ============ Utilities ============
function formatSize(bytes: number): string {
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
  if (bytes < 1024 * 1024 * 1024) return (bytes / 1024 / 1024).toFixed(1) + ' MB';
  return (bytes / 1024 / 1024 / 1024).toFixed(2) + ' GB';
}

function formatTime(ts: number | null): string {
  if (!ts) return '-';
  return new Date(ts * 1000).toLocaleString('zh-CN');
}

function getStatusText(status: string): string {
  switch (status) {
    case 'Pending': return '待确认';
    case 'Transferring': return '传输中';
    case 'Completed': return '已完成';
    case 'Failed': return '失败';
    case 'Cancelled': return '已取消';
    default: return status;
  }
}

type Tab = 'device' | 'share' | 'download';

// ============ App ============
function App() {
  const [tab, setTab] = useState<Tab>('device');
  const [deviceName, setDeviceName] = useState('');
  const [version] = useState('1.0.0-beta.3');
  const [localIp, setLocalIp] = useState('');
  const [localPort, setLocalPort] = useState(18766);

  // 设备相关状态
  const [devices, setDevices] = useState<Device[]>([]);
  const [selectedDevice, setSelectedDevice] = useState<Device | null>(null);
  const [transfers, setTransfers] = useState<TransferTask[]>([]);
  const [peerIp, setPeerIp] = useState('');
  const [peerPort, setPeerPort] = useState('18766');
  const [isAdding, setIsAdding] = useState(false);
  const [isScanning, setIsScanning] = useState(false);
  const [pendingRequests, setPendingRequests] = useState<PendingRequest[]>([]);
  const [showReceiveModal, setShowReceiveModal] = useState(false);

  // 共享目录状态
  const [sharedDirs, setSharedDirs] = useState<SharedDir[]>([]);
  const [isSharing, setIsSharing] = useState(false);

  // 局域网下载状态
  const [hosts, setHosts] = useState<HostInfo[]>([]);
  const [expandedHost, setExpandedHost] = useState<string | null>(null);
  const [expandedDir, setExpandedDir] = useState<{ hostId: string; dirId: string } | null>(null);
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [breadcrumbs, setBreadcrumbs] = useState<{ hostId: string; dirId: string; name: string }[]>([]);
  const [downloads, setDownloads] = useState<DownloadProgress[]>([]);
  const [isDiscovering, setIsDiscovering] = useState(false);
  const [downloadDir, setDownloadDir] = useState('');

  // 设备 Tab - 共享浏览状态（集成在设备 Tab 内）
  const [browsingHost, setBrowsingHost] = useState<HostInfo | null>(null);
  const [browsingFiles, setBrowsingFiles] = useState<FileEntry[]>([]);
  const [browsingBreadcrumbs, setBrowsingBreadcrumbs] = useState<{ hostId: string; dirId: string; name: string }[]>([]);
  const [browsingIp, setBrowsingIp] = useState<string>(''); // 保存浏览时的 IP（对方 HostInfo 里没有 ip）

  // ============ 通用初始化 ============
  useEffect(() => {
    const init = async () => {
      try {
        await new Promise(resolve => setTimeout(resolve, 500));
        try {
          const name = await invoke<string>('get_device_name');
          const [ip, port] = await invoke<[string, number]>('get_local_info');
          setDeviceName(name);
          setLocalIp(ip);
          setLocalPort(port);
        } catch (e) {
          setLocalIp('初始化中...');
        }
      } catch (e) {
        console.error('Init failed:', e);
      }
    };
    init();
  }, []);

  // ============ 设备列表轮询 ============
  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const devs = await invoke<Device[]>('get_devices');
        setDevices(devs);
      } catch (e) {}
    }, 2000);
    return () => clearInterval(interval);
  }, []);

  // ============ 待接收文件轮询 ============
  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const pending = await invoke<PendingRequest[]>('get_pending_requests');
        if (pending.length > 0) {
          setPendingRequests(pending);
          setShowReceiveModal(true);
        } else {
          setShowReceiveModal(false);
        }
      } catch (e) {}
    }, 1000);
    return () => clearInterval(interval);
  }, []);

  // ============ 传输进度轮询 ============
  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const progress = await invoke<TransferTask[]>('get_transfer_progress');
        setTransfers(progress);
      } catch (e) {}
    }, 500);
    return () => clearInterval(interval);
  }, []);

  // ============ 共享目录轮询 ============
  useEffect(() => {
    const loadSharedDirs = async () => {
      try {
        const dirs = await invoke<SharedDir[]>('get_shared_dirs');
        setSharedDirs(dirs);
      } catch (e) {
        console.error('Failed to load shared dirs:', e);
      }
    };
    loadSharedDirs();
    const interval = setInterval(loadSharedDirs, 3000);
    return () => clearInterval(interval);
  }, []);

  // ============ 设备相关 ============
  const handleNameChange = async (newName: string) => {
    try {
      await invoke('set_device_name', { name: newName });
      setDeviceName(newName);
    } catch (e) {
      console.error('Failed to set name:', e);
    }
  };

  const handleAddPeer = async () => {
    if (!peerIp.trim()) return;
    setIsAdding(true);
    try {
      const port = parseInt(peerPort) || 18766;
      const device = await invoke<Device>('add_peer', { peerIp: peerIp.trim(), peerPort: port });
      setDevices(prev => [...prev.filter(d => d.id !== device.id), device]);
      setSelectedDevice(device);
      setPeerIp('');
    } catch (e) {
      alert('连接失败：' + e);
    } finally {
      setIsAdding(false);
    }
  };

  const handleScan = async () => {
    setIsScanning(true);
    try {
      const found = await invoke<Device[]>('scan_subnet', { 'port': 18766 });
      if (found.length === 0) {
        alert('未发现任何设备');
      } else {
        setDevices(prev => {
          const newList = [...prev];
          for (const device of found) {
            if (!newList.some(d => d.id === device.id)) newList.push(device);
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

  const handleSendFile = async () => {
    if (!selectedDevice) return;
    try {
      const filePath = await open({ multiple: false, directory: false, title: '选择要发送的文件' });
      if (!filePath) return;
      await invoke('send_file', { targetIp: selectedDevice.ip, targetPort: selectedDevice.port, filePath: filePath });
      alert(`文件已发送给 ${selectedDevice.name}`);
    } catch (e) {
      alert('发送失败：' + e);
    }
  };

  const handleSendFolder = async () => {
    if (!selectedDevice) return;
    try {
      const folderPath = await open({ directory: true, title: '选择要发送的文件夹' });
      if (!folderPath) return;
      await invoke('send_file', { targetIp: selectedDevice.ip, targetPort: selectedDevice.port, filePath: folderPath });
      alert(`文件夹已发送给 ${selectedDevice.name}`);
    } catch (e) {
      alert('发送失败：' + e);
    }
  };

  // 访问选中设备的共享目录
  const handleAccessSharedDirs = async () => {
    if (!selectedDevice) return;
    try {
      setBrowsingHost(null);
      setBrowsingFiles([]);
      setBrowsingBreadcrumbs([]);
      setBrowsingIp(selectedDevice.ip); // 保存 IP
      // 通过 Rust 端 HTTP 请求获取对方共享目录列表（绕过 CORS）
      const host = await invoke<HostInfo>('fetch_shared_dirs', { hostIp: selectedDevice.ip });
      if (!host.shared_dirs || host.shared_dirs.length === 0) {
        alert('该设备没有共享目录');
        return;
      }
      setBrowsingHost(host);
    } catch (e) {
      console.error('fetch_shared_dirs error:', e);
      alert('访问共享目录失败：' + e);
    }
  };

  // 浏览共享目录内的子目录或下载文件
  const handleBrowseDir = async (host: HostInfo, dirId: string, dirName: string, currentPath: string) => {
    try {
      // 完整路径：从面包屑重建
      const fullPath = currentPath ? `${currentPath}/${dirName}` : dirName;
      const data = await invoke<FileEntry[]>('browse_remote_dir', {
        hostIp: browsingIp,
        dirId: dirId,
        dirPath: fullPath
      });
      setBrowsingHost(host);
      setBrowsingFiles(data);
      setBrowsingBreadcrumbs(prev => {
        // 如果已在路径中（点击了同级目录），回退到该位置
        const idx = prev.findIndex(b => b.name === dirName);
        if (idx >= 0) return prev.slice(0, idx + 1);
        return [...prev, { hostId: host.id, dirId, name: dirName }];
      });
    } catch (e) {
      console.error('Failed to browse dir:', e);
      alert('浏览失败：' + e);
      setBrowsingFiles([]);
    }
  };

  // 下载共享文件
  const handleDownloadSharedFile = async (dirId: string, file: FileEntry, currentPath: string) => {
    const file_path = currentPath ? `${currentPath}/${file.name}` : file.name;
    const save_dir = downloadDir;
    const downloadId = `dl-${Date.now()}`;
    setDownloads(prev => [...prev, { id: downloadId, file_name: file.name, total: file.size, downloaded: 0, progress: 0 }]);
    try {
      const savedPath = await invoke<string>('download_file', {
        hostIp: browsingIp,
        dirId: dirId,
        filePath: file_path,
        saveDir: save_dir
      });
      setDownloads(prev => prev.filter(d => d.id !== downloadId));
      alert(`下载完成：${savedPath}`);
    } catch (e) {
      setDownloads(prev => prev.filter(d => d.id !== downloadId));
      alert(`下载失败：${e}`);
    }
  };

  // 下载整个目录（递归）
  const handleDownloadSharedDir = async (dirId: string, dirName: string, currentPath: string) => {
    const dir_path = currentPath ? `${currentPath}/${dirName}` : dirName;
    const save_dir = downloadDir;

    // 先统计文件数量
    let fileCount = 0;
    try {
      fileCount = await invoke<number>('count_remote_dir_files', {
        hostIp: browsingIp,
        dirId: dirId,
        dirPath: dir_path
      });
    } catch (e) {
      alert(`无法统计目录文件数：${e}`);
      return;
    }

    // 超过100个文件时弹确认框
    if (fileCount > 100) {
      if (!window.confirm(`该目录包含 ${fileCount} 个文件，是否继续下载？`)) {
        return;
      }
    }

    const downloadId = `dl-${Date.now()}`;
    setDownloads(prev => [...prev, {
      id: downloadId,
      file_name: dirName,
      total: 0,
      downloaded: 0,
      progress: 0,
      is_dir: true,
      file_count: fileCount
    }]);

    try {
      const [count, bytes] = await invoke<[number, number]>('download_dir', {
        hostIp: browsingIp,
        dirId: dirId,
        dirPath: dir_path,
        dirName: dirName,
        saveDir: save_dir
      });
      setDownloads(prev => prev.filter(d => d.id !== downloadId));
      alert(`下载完成：${dirName}（${count} 个文件，${formatSize(bytes)}）`);
    } catch (e) {
      setDownloads(prev => prev.filter(d => d.id !== downloadId));
      alert(`下载失败：${e}`);
    }
  };

  // 返回共享目录列表
  // 返回上一级（如果有上级目录）
  const handleBack = async () => {
    if (browsingBreadcrumbs.length <= 1) {
      // 已经是最顶层，返回共享目录列表
      setBrowsingFiles([]);
      setBrowsingBreadcrumbs([]);
    } else {
      // 弹出最后一个面包屑，返回上级目录
      const newBreadcrumbs = browsingBreadcrumbs.slice(0, -1);
      const parentBreadcrumb = newBreadcrumbs[newBreadcrumbs.length - 1];
      // 重新浏览上级目录
      try {
        const data = await invoke<FileEntry[]>('browse_remote_dir', {
          hostIp: browsingIp,
          dirId: parentBreadcrumb.dirId,
          dirPath: newBreadcrumbs.map(b => b.name).join('/')
        });
        setBrowsingBreadcrumbs(newBreadcrumbs);
        setBrowsingFiles(data);
      } catch (e) {
        alert('返回失败：' + e);
      }
    }
  };

  // 返回共享目录列表
  const handleBackToSharedDirs = () => {
    setBrowsingHost(null);
    setBrowsingFiles([]);
    setBrowsingBreadcrumbs([]);
  };

  const handleAcceptFile = async (reqId: string) => {
    setShowReceiveModal(false);
    setPendingRequests([]);
    try {
      await invoke('confirm_receive', { req_id: reqId, save_path: null });
    } catch (e) {
      alert('接收失败：' + String(e));
    }
  };

  const handleAcceptFileAs = async (reqId: string) => {
    try {
      const path = await open({ directory: true, title: '选择保存位置' });
      if (!path) return;
      setShowReceiveModal(false);
      setPendingRequests([]);
      await invoke('confirm_receive', { req_id: reqId, save_path: path });
    } catch (e) {
      alert('接收失败：' + String(e));
    }
  };

  const handleRejectFile = async (reqId: string) => {
    try {
      await invoke('reject_receive', { req_id: reqId });
      setShowReceiveModal(false);
      setPendingRequests([]);
    } catch (e) {
      alert('操作失败：' + String(e));
    }
  };

  // ============ 共享目录管理 ============
  const handleAddSharedDir = async () => {
    try {
      const dirPath = await open({ directory: true, title: '选择要共享的目录' });
      if (!dirPath) return;
      // 从路径中提取目录名
      const name = dirPath.split(/[/\\]/).pop() || dirPath;
      await invoke('add_shared_dir', { name, path: dirPath });
      // 刷新列表
      const dirs = await invoke<SharedDir[]>('get_shared_dirs');
      setSharedDirs(dirs);
    } catch (e) {
      alert('添加共享目录失败：' + e);
    }
  };

  const handleRemoveSharedDir = async (dirId: string) => {
    try {
      await invoke('remove_shared_dir', { dir_id: dirId });
      setSharedDirs(prev => prev.filter(d => d.id !== dirId));
    } catch (e) {
      alert('移除共享目录失败：' + e);
    }
  };

  const handleToggleSharing = async () => {
    if (isSharing) {
      try {
        await invoke('stop_discovery');
        setIsSharing(false);
      } catch (e) {
        alert('停止共享失败：' + e);
      }
    } else {
      if (sharedDirs.length === 0) {
        alert('请先添加至少一个共享目录');
        return;
      }
      try {
        await invoke('start_discovery', {
          name: deviceName,
          port: 18767,
          shared_dirs: sharedDirs.map(d => ({ id: d.id, name: d.name, path: d.path }))
        });
        setIsSharing(true);
      } catch (e) {
        alert('开启共享失败：' + e);
      }
    }
  };

  // ============ 局域网下载 ============
  const handleDiscoverHosts = async () => {
    setIsDiscovering(true);
    setHosts([]);
    try {
      const found = await invoke<HostInfo[]>('discover_hosts', { timeout_ms: 3000 });
      setHosts(found);
    } catch (e) {
      alert('扫描失败：' + e);
    } finally {
      setIsDiscovering(false);
    }
  };

  const handleExpandHost = async (host: HostInfo) => {
    if (expandedHost === host.id) {
      setExpandedHost(null);
    } else {
      setExpandedHost(host.id);
    }
  };

  const handleExpandDir = async (host: HostInfo, dir: SharedDir) => {
    if (expandedDir?.hostId === host.id && expandedDir?.dirId === dir.id) {
      setExpandedDir(null);
      setFiles([]);
      setBreadcrumbs([]);
    } else {
      setExpandedDir({ hostId: host.id, dirId: dir.id });
      setBreadcrumbs([{ hostId: host.id, dirId: dir.id, name: dir.name }]);
      try {
        const res = await fetch(`http://${host.ip}:18767/d/${dir.id}`);
        if (res.ok) {
          const data: FileEntry[] = await res.json();
          setFiles(data);
        } else {
          setFiles([]);
        }
      } catch (e) {
        console.error('Failed to fetch dir:', e);
        setFiles([]);
      }
    }
  };

  const handleNavigateDir = async (hostIp: string, dirId: string, dirName: string, currentPath: string) => {
    setBreadcrumbs(prev => {
      const idx = prev.findIndex(b => b.name === dirName);
      if (idx >= 0) return prev.slice(0, idx + 1);
      return [...prev, { hostId: expandedDir!.hostId, dirId, name: dirName }];
    });
    setExpandedDir({ hostId: expandedDir!.hostId, dirId });
    try {
      const path = currentPath ? `${currentPath}/${dirName}` : dirName;
      const url = `http://${hostIp}:18767/d/${dirId}/${encodeURIComponent(path)}`;
      const res = await fetch(url);
      if (res.ok) {
        const data: FileEntry[] = await res.json();
        setFiles(data);
      } else {
        setFiles([]);
      }
    } catch (e) {
      console.error('Failed to navigate dir:', e);
      setFiles([]);
    }
  };

  const handleSelectDownloadDir = async () => {
    try {
      const dir = await open({ directory: true, title: '选择下载保存目录' });
      if (dir) {
        setDownloadDir(dir);
      }
    } catch (e) {
      console.error('Failed to select dir:', e);
    }
  };

  const handleDownloadFile = async (hostIp: string, dirId: string, file: FileEntry) => {
    // 构建相对路径
    const file_path = file.path;
    // 目标保存目录：用户选择优先，否则为空（由 Rust 端使用系统下载目录）
    const save_dir = downloadDir;

    const downloadId = `dl-${Date.now()}`;
    setDownloads(prev => [...prev, {
      id: downloadId,
      file_name: file.name,
      total: file.size,
      downloaded: 0,
      progress: 0
    }]);

    try {
      // 调用 Rust 下载命令
      const savedPath = await invoke<string>('download_file', {
        hostIp: hostIp,
        dirId: dirId,
        filePath: file_path,
        saveDir: save_dir
      });

      // 下载完成，移除进度
      setDownloads(prev => prev.filter(d => d.id !== downloadId));
      alert(`下载完成：${savedPath}`);
    } catch (e) {
      setDownloads(prev => prev.filter(d => d.id !== downloadId));
      alert(`下载失败：${e}`);
    }
  };

  const handleBackToDirs = () => {
    setExpandedDir(null);
    setFiles([]);
    setBreadcrumbs([]);
  };

  return (
    <div className="app">
      {/* Header */}
      <header className="header">
        <h1>📡 隔空投送 <span className="version">{version}</span></h1>
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

      {/* Tab Navigation */}
      <nav className="tab-nav">
        <button className={`tab-btn ${tab === 'device' ? 'active' : ''}`} onClick={() => setTab('device')}>
          💻 设备
        </button>
        <button className={`tab-btn ${tab === 'share' ? 'active' : ''}`} onClick={() => setTab('share')}>
          📂 共享
        </button>
        <button className={`tab-btn ${tab === 'download' ? 'active' : ''}`} onClick={() => setTab('download')}>
          ⬇️ 下载
        </button>
      </nav>

      <main className="main">
        {/* ============ 设备 Tab ============ */}
        {tab === 'device' && (
          <>
            <section className="devices-section">
              <h2>在线设备</h2>
              <div className="add-peer-form">
                <h3>添加主机</h3>
                <div className="form-row">
                  <input type="text" value={peerIp} onChange={(e) => setPeerIp(e.target.value)}
                    placeholder="对方IP，如 192.168.1.100" className="peer-input" />
                  <input type="text" value={peerPort} onChange={(e) => setPeerPort(e.target.value)}
                    placeholder="端口" className="port-input" />
                  <button className="add-btn" onClick={handleAddPeer}
                    disabled={isAdding || !peerIp.trim()}>
                    {isAdding ? '连接中...' : '添加'}
                  </button>
                </div>
                <div className="form-row" style={{ marginTop: '12px' }}>
                  <button className="scan-btn" onClick={handleScan} disabled={isScanning}>
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
                    <div key={device.id}
                      className={`device-card ${selectedDevice?.id === device.id ? 'selected' : ''}`}
                      onClick={() => setSelectedDevice(device)}>
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
                  <div className="action-buttons">
                    <button className="send-btn" onClick={handleSendFile}>📄 发文件</button>
                    <button className="send-btn send-folder-btn" onClick={handleSendFolder}>📂 发文件夹</button>
                    <button className="send-btn access-shared-btn" onClick={handleAccessSharedDirs}>🌐 访问共享</button>
                  </div>
                </div>
              ) : (
                <div className="empty-state"><p>请先添加并选择接收设备</p></div>
              )}

              {/* 共享目录浏览器（集成在设备 Tab） */}
              {browsingHost && (
                <div className="shared-browser">
                  <h3>🌐 {browsingHost.name} 的共享目录</h3>
                  {/* 面包屑 */}
                  {browsingBreadcrumbs.length > 0 && (
                    <div className="breadcrumbs">
                      <button className="back-btn" onClick={browsingFiles.length === 0 ? handleBackToSharedDirs : handleBack}>
                        ← {browsingFiles.length === 0 ? '返回' : '返回上一级'}
                      </button>
                      <span className="breadcrumb-path">
                        {browsingBreadcrumbs.map((b, i) => (
                          <span key={i}>
                            <span className="breadcrumb-sep"> / </span>
                            <span className="breadcrumb-item">{b.name}</span>
                          </span>
                        ))}
                      </span>
                    </div>
                  )}
                  {/* 共享目录列表或文件列表 */}
                  {browsingFiles.length === 0 ? (
                    <div className="shared-dir-list">
                      {browsingHost.shared_dirs.map((dir) => (
                        <div key={dir.id} className="shared-dir-item" onClick={() => handleBrowseDir(browsingHost, dir.id, dir.name, '')}>
                          <span className="dir-icon">📁</span>
                          <div className="dir-info">
                            <span className="dir-name">{dir.name}</span>
                            <span className="dir-path">{dir.path}</span>
                          </div>
                        </div>
                      ))}
                    </div>
                  ) : (
                    <div className="file-list">
                      {browsingFiles.map((file, idx) => {
                        // 当前所在的共享目录 id（面包屑第一项）
                        const rootDirId = browsingBreadcrumbs.length > 0 ? browsingBreadcrumbs[0].dirId : browsingHost.shared_dirs[0]?.id || '';
                        // 当前路径（面包屑累积的路径）
                        const currentPath = browsingBreadcrumbs.length > 0 ? browsingBreadcrumbs[browsingBreadcrumbs.length - 1].name : '';
                        return (
                          <div key={idx} className="file-row">
                            <span className="file-icon">{file.is_dir ? '📁' : '📄'}</span>
                            <span className="file-name" onClick={() => file.is_dir && rootDirId && handleBrowseDir(browsingHost, rootDirId, file.name, currentPath)} style={{ cursor: file.is_dir ? 'pointer' : 'default' }}>{file.name}</span>
                            <span className="file-size">{file.is_dir ? '-' : formatSize(file.size)}</span>
                            {file.is_dir && rootDirId && (
                              <button className="file-download-btn" onClick={() => handleDownloadSharedDir(rootDirId, file.name, currentPath)}>⬇️</button>
                            )}
                            {!file.is_dir && rootDirId && (
                              <button className="file-download-btn" onClick={() => handleDownloadSharedFile(rootDirId, file, currentPath)}>⬇️</button>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              )}

              <div className="transfer-list">
                <h3>传输进度</h3>
                {transfers.length === 0 ? (
                  <p className="hint">暂无传输</p>
                ) : (
                  transfers.map((t) => (
                    <div key={t.id} className={`transfer-item ${t.status === 'Transferring' ? 'transferring' : ''}`}>
                      <div className="transfer-info">
                        <span className="transfer-name">
                          {t.direction === 'Send' ? '→' : '←'} {t.file_name}
                        </span>
                        <span className={`transfer-status ${t.status.toLowerCase()}`}>
                          {getStatusText(t.status)}
                        </span>
                      </div>
                      <div className="progress-bar">
                        <div className={`progress-fill ${t.direction === 'Send' ? 'send' : 'receive'}`}
                          style={{ width: `${t.progress}%` }}></div>
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
          </>
        )}

        {/* ============ 共享 Tab ============ */}
        {tab === 'share' && (
          <section className="share-section">
            <h2>📂 共享目录管理</h2>
            <p className="hint" style={{ marginBottom: '16px' }}>
              添加本地目录供局域网内其他设备下载
            </p>

            {/* 共享开关 */}
            <div className="share-toggle-panel">
              <div className="share-toggle-info">
                <span className="share-toggle-label">🛰️ 局域网共享服务</span>
                <span className={`share-toggle-status ${isSharing ? 'active' : ''}`}>
                  {isSharing ? '已开启（其他设备可发现并下载）' : '已停止'}
                </span>
              </div>
              <button
                className={`toggle-btn ${isSharing ? 'stop' : 'start'}`}
                onClick={handleToggleSharing}
              >
                {isSharing ? '⏹ 停止共享' : '▶ 开启共享'}
              </button>
            </div>

            {/* 共享目录列表 */}
            <div className="share-dir-header">
              <h3>我的共享目录（{sharedDirs.length}）</h3>
              <button className="add-share-btn" onClick={handleAddSharedDir}>
                ➕ 添加共享目录
              </button>
            </div>

            {sharedDirs.length === 0 ? (
              <div className="empty-state">
                <p>暂无共享目录</p>
                <p className="hint">点击上方"添加共享目录"选择要分享的文件夹</p>
              </div>
            ) : (
              <div className="share-dir-list">
                {sharedDirs.map((dir) => (
                  <div key={dir.id} className="share-dir-card">
                    <div className="share-dir-icon">📁</div>
                    <div className="share-dir-info">
                      <div className="share-dir-name">{dir.name}</div>
                      <div className="share-dir-path">{dir.path}</div>
                    </div>
                    <button
                      className="remove-share-btn"
                      onClick={() => handleRemoveSharedDir(dir.id)}
                      title="移除共享"
                    >
                      🗑️
                    </button>
                  </div>
                ))}
              </div>
            )}

            {/* 本机共享信息 */}
            {isSharing && (
              <div className="share-info-panel">
                <h3>🖥️ 本机共享信息</h3>
                <div className="share-info-grid">
                  <div className="share-info-item">
                    <span className="info-label">设备名称</span>
                    <span className="info-value">{deviceName}</span>
                  </div>
                  <div className="share-info-item">
                    <span className="info-label">HTTP 端口</span>
                    <span className="info-value">18767</span>
                  </div>
                  <div className="share-info-item">
                    <span className="info-label">本机 IP</span>
                    <span className="info-value">{localIp}</span>
                  </div>
                  <div className="share-info-item">
                    <span className="info-label">共享地址</span>
                    <span className="info-value copyable">http://{localIp}:18767</span>
                  </div>
                </div>
              </div>
            )}
          </section>
        )}

        {/* ============ 下载 Tab ============ */}
        {tab === 'download' && (
          <section className="download-section">
            <h2>⬇️ 局域网下载</h2>

            {/* 扫描按钮 */}
            <button className="discover-btn" onClick={handleDiscoverHosts} disabled={isDiscovering}>
              {isDiscovering ? '🔍 扫描中...' : '🔍 扫描局域网设备'}
            </button>

            {/* 下载目录选择 */}
            <div className="download-dir-selector">
              <span className="download-dir-label">📥 保存到：</span>
              <span className="download-dir-path">
                {downloadDir || '（默认：系统下载目录/FeigeTransfer）'}
              </span>
              <button className="download-dir-btn" onClick={handleSelectDownloadDir}>
                📂 更改目录
              </button>
            </div>

            {/* 活跃下载 */}
            {downloads.length > 0 && (
              <div className="active-downloads">
                <h3>📥 正在下载（{downloads.length}）</h3>
                {downloads.map((dl) => (
                  <div key={dl.id} className="download-item downloading">
                    <div className="download-info">
                      <span className="download-name">{dl.is_dir ? '📁' : '📄'} {dl.file_name}</span>
                      <span className="download-size">
                        {dl.is_dir
                          ? `${dl.file_count || 0} 个文件`
                          : `${formatSize(dl.downloaded)} / ${formatSize(dl.total)}`}
                      </span>
                    </div>
                    <div className="progress-bar">
                      <div className="progress-fill receive" style={{ width: '100%' }}></div>
                    </div>
                  </div>
                ))}
              </div>
            )}

            {/* 设备列表 */}
            {hosts.length === 0 ? (
              <div className="empty-state">
                <p>未发现共享设备</p>
                <p className="hint">点击上方"扫描局域网设备"开始搜索</p>
              </div>
            ) : (
              <div className="host-list">
                {hosts.map((host) => (
                  <div key={host.id} className="host-card">
                    {/* 主机行 */}
                    <div
                      className={`host-header ${expandedHost === host.id ? 'expanded' : ''}`}
                      onClick={() => handleExpandHost(host)}
                    >
                      <span className="host-expand-icon">{expandedHost === host.id ? '▼' : '▶'}</span>
                      <span className="host-icon">🖥️</span>
                      <div className="host-info">
                        <span className="host-name">{host.name}</span>
                        <span className="host-meta">http://{host.ip}:18767 · {host.shared_dirs.length} 个共享目录</span>
                      </div>
                    </div>

                    {/* 共享目录列表 */}
                    {expandedHost === host.id && (
                      <div className="host-dirs">
                        {host.shared_dirs.map((dir) => {
                          const isDirActive = expandedDir?.hostId === host.id && expandedDir?.dirId === dir.id;
                          return (
                            <div key={dir.id} className="host-dir-item">
                              <div
                                className={`dir-row ${isDirActive ? 'active' : ''}`}
                                onClick={() => handleExpandDir(host, dir)}
                              >
                                <span className="dir-expand-icon">{isDirActive ? '▼' : '▶'}</span>
                                <span className="dir-icon">📁</span>
                                <span className="dir-name">{dir.name}</span>
                                <span className="dir-path">{dir.path}</span>
                              </div>

                              {/* 文件浏览器 */}
                              {isDirActive && (
                                <div className="file-browser">
                                  {/* 面包屑 */}
                                  <div className="breadcrumbs">
                                    <button className="back-btn" onClick={handleBackToDirs}>← 返回</button>
                                    <span className="breadcrumb-path">
                                      {breadcrumbs.map((b, i) => (
                                        <span key={i}>
                                          <span className="breadcrumb-sep"> / </span>
                                          <span className="breadcrumb-item">{b.name}</span>
                                        </span>
                                      ))}
                                    </span>
                                  </div>

                                  {/* 文件列表 */}
                                  {files.length === 0 ? (
                                    <div className="empty-dir">
                                      <p>空目录或加载失败</p>
                                    </div>
                                  ) : (
                                    <div className="file-list">
                                      {files.map((file, idx) => (
                                        <div key={idx} className="file-row">
                                          <span className="file-icon">{file.is_dir ? '📁' : '📄'}</span>
                                          <span className="file-name"
                                            onClick={() => {
                                              if (file.is_dir) {
                                                handleNavigateDir(host.ip, dir.id, file.name, file.path);
                                              }
                                            }}
                                            style={{ cursor: file.is_dir ? 'pointer' : 'default' }}
                                          >
                                            {file.name}
                                          </span>
                                          <span className="file-size">{file.is_dir ? '-' : formatSize(file.size)}</span>
                                          <span className="file-modified">{formatTime(file.modified ?? null)}</span>
                                          {!file.is_dir && (
                                            <button
                                              className="file-download-btn"
                                              onClick={() => handleDownloadFile(host.ip, dir.id, file)}
                                            >
                                              ⬇️ 下载
                                            </button>
                                          )}
                                        </div>
                                      ))}
                                    </div>
                                  )}
                                </div>
                              )}
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </section>
        )}
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
              <button className="modal-btn accept-btn"
                onClick={() => handleAcceptFile(pendingRequests[0].id)}>
                📥 接收（默认路径）
              </button>
              <button className="modal-btn accept-as-btn"
                onClick={() => handleAcceptFileAs(pendingRequests[0].id)}>
                📂 另存为...
              </button>
              <button className="modal-btn reject-btn"
                onClick={() => handleRejectFile(pendingRequests[0].id)}>
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
