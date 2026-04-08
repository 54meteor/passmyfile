use anyhow::Result;
use http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use log::{error, info};
use mime_guess::MimeGuess;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

use crate::types::{FileEntry, HostInfo, SharedDir};
use crate::write_log;

const HTTP_PORT: u16 = 18767;
const CHUNK_SIZE: usize = 64 * 1024; // 64KB

/// 解析后的 HTTP 请求
struct HttpRequest {
    method: String,
    path: String,
    headers: HeaderMap,
}

impl HttpRequest {
    fn parse(request_bytes: &[u8]) -> Option<Self> {
        let request_str = String::from_utf8_lossy(request_bytes).to_string();
        let mut lines = request_str.lines();

        // 请求行: GET /path HTTP/1.1
        let request_line = lines.next()?;
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }
        let method = parts[0].to_string();
        let path = parts[1].to_string();

        // 解析 headers
        let mut headers = HeaderMap::new();
        for line in lines.by_ref() {
            if line.is_empty() {
                break;
            }
            if let Some(colon_pos) = line.find(':') {
                let name = line[..colon_pos].trim();
                let value = line[colon_pos + 1..].trim();
                if let (Ok(name), Ok(value)) = (
                    HeaderName::from_str(name),
                    HeaderValue::from_str(value),
                ) {
                    headers.insert(name, value);
                }
            }
        }

        Some(HttpRequest { method, path, headers })
    }
}

/// HTTP Range 解析结果
#[derive(Debug)]
pub struct RangeSpec {
    pub start: u64,
    pub end: Option<u64>,
}

/// HTTP 文件服务器
pub struct HttpFileServer {
    /// 共享目录列表 key=dir_id, value=SharedDir
    shared_dirs: Arc<RwLock<HashMap<String, SharedDir>>>,
    /// 主机信息
    host_info: Arc<RwLock<HostInfo>>,
    /// 下载使用的目录（与 transfer service 共享下载目录）
    downloads_dir: PathBuf,
}

impl HttpFileServer {
    /// 创建 HTTP 文件服务器
    pub fn new(downloads_dir: PathBuf, device_id: String, device_name: String) -> Self {
        let host_info = HostInfo {
            id: device_id,
            name: device_name,
            ip: String::new(),
            port: HTTP_PORT,
            shared_dirs: Vec::new(),
        };

        Self {
            shared_dirs: Arc::new(RwLock::new(HashMap::new())),
            host_info: Arc::new(RwLock::new(host_info)),
            downloads_dir,
        }
    }

    /// 添加共享目录
    pub async fn add_shared_dir(&self, name: String, path: PathBuf) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let path_str = path.to_string_lossy().to_string();
        let dir = SharedDir {
            id: id.clone(),
            name,
            path: path_str,
        };

        if !path.exists() || !path.is_dir() {
            write_log(&format!("[HttpServer] Shared dir path invalid: {:?}", path));
        }

        {
            let mut dirs = self.shared_dirs.write().await;
            dirs.insert(id.clone(), dir);
        }

        let mut host = self.host_info.write().await;
        host.shared_dirs = self.shared_dirs.read().await.values().cloned().collect();

        write_log(&format!("[HttpServer] Added shared dir: {} -> {:?}", id, path));
        id
    }

    /// 移除共享目录
    pub async fn remove_shared_dir(&self, dir_id: &str) {
        {
            let mut dirs = self.shared_dirs.write().await;
            dirs.remove(dir_id);
        }
        let mut host = self.host_info.write().await;
        host.shared_dirs = self.shared_dirs.read().await.values().cloned().collect();
    }

    /// 获取共享目录列表
    pub async fn get_shared_dirs(&self) -> Vec<SharedDir> {
        let dirs = self.shared_dirs.read().await;
        dirs.values().cloned().collect()
    }

    /// 启动 HTTP 服务器
    pub async fn start(&self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", HTTP_PORT);
        let listener = TcpListener::bind(&addr).await?;
        write_log(&format!("[HttpServer] Started on http://0.0.0.0:{}/", HTTP_PORT));
        info!("[HttpServer] Listening on {}", addr);

        let shared_dirs = self.shared_dirs.clone();
        let host_info = self.host_info.clone();
        let downloads_dir = self.downloads_dir.clone();

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        let shared_dirs = shared_dirs.clone();
                        let host_info = host_info.clone();
                        let downloads_dir = downloads_dir.clone();

                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, addr, shared_dirs, host_info, downloads_dir).await {
                                error!("[HttpServer] Connection error from {}: {}", addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("[HttpServer] Accept error: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            }
        });

        Ok(())
    }
}

/// 处理单个 HTTP 连接
async fn handle_connection(
    mut stream: TcpStream,
    addr: std::net::SocketAddr,
    shared_dirs: Arc<RwLock<HashMap<String, SharedDir>>>,
    host_info: Arc<RwLock<HostInfo>>,
    downloads_dir: PathBuf,
) -> Result<()> {
    // 读取完整请求
    let mut request_bytes = vec![0u8; 65536];
    let n = stream.read(&mut request_bytes).await?;
    if n == 0 {
        return Ok(());
    }
    request_bytes.truncate(n);

    let request = match HttpRequest::parse(&request_bytes) {
        Some(r) => r,
        None => {
            send_error(&mut stream, StatusCode::BAD_REQUEST, "Bad Request").await?;
            return Ok(());
        }
    };

    // 只支持 GET
    if request.method != "GET" {
        send_error(&mut stream, StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed").await?;
        return Ok(());
    }

    write_log(&format!("[HttpServer] GET {} from {}", request.path, addr));

    // 路由分发
    if request.path == "/" {
        handle_root(&mut stream, host_info).await?;
    } else if request.path.starts_with("/d/") {
        let remaining = &request.path[3..];
        handle_dir_or_file(&mut stream, remaining, &request.headers, &shared_dirs, &downloads_dir).await?;
    } else {
        send_error(&mut stream, StatusCode::NOT_FOUND, "Not Found").await?;
    }

    Ok(())
}

/// 处理 GET / - 返回主机信息和共享目录列表
async fn handle_root(
    stream: &mut TcpStream,
    host_info: Arc<RwLock<HostInfo>>,
) -> Result<()> {
    let info = host_info.read().await;
    let body = serde_json::to_string_pretty(&*info)?;
    write_response(stream, StatusCode::OK, "application/json; charset=utf-8", body.as_bytes(), false, None).await
}

/// 处理 GET /d/<dir_id>[/<path>]
async fn handle_dir_or_file(
    stream: &mut TcpStream,
    remaining: &str,
    headers: &HeaderMap,
    shared_dirs: &Arc<RwLock<HashMap<String, SharedDir>>>,
    downloads_dir: &PathBuf,
) -> Result<()> {
    // 解析 dir_id 和 path
    let parts: Vec<&str> = remaining.splitn(2, '/').collect();
    let dir_id = parts[0];
    let request_path = parts.get(1).unwrap_or(&"");

    // 获取共享目录
    let dir = {
        let dirs = shared_dirs.read().await;
        dirs.get(dir_id).cloned()
    };

    let dir = match dir {
        Some(d) => d,
        None => {
            send_error(stream, StatusCode::NOT_FOUND, "Shared directory not found").await?;
            return Ok(());
        }
    };

    // 构建目标路径（将 String 转为 PathBuf）
    let dir_path = std::path::PathBuf::from(&dir.path);
    let target = if request_path.is_empty() {
        dir_path.clone()
    } else {
        // URL 解码请求路径（如 "%2F" → "/"）
        let decoded_path = urlencoding::decode(request_path)
            .map(|s| s.to_string())
            .unwrap_or_else(|_| request_path.to_string());
        // 规范化请求路径：转换斜杠为反斜杠
        let normalized_path = decoded_path.replace('/', "\\");
        // Windows 上 PathBuf::join 把裸盘符 "D:" 当相对路径，拼成 "base\D:"。
        // 需要特殊处理：裸盘符 → "D:\"，绝对路径 "D:\path" 直接用，
        // 其他相对路径正常 join
        let use_path = if normalized_path.len() == 2 && normalized_path.chars().nth(1) == Some(':') {
            // 裸盘符 "D:" → "D:\"（绝对路径）
            format!("{}\\", normalized_path)
        } else {
            normalized_path
        };
        let requested_full = if use_path.len() >= 3 && use_path.chars().nth(1) == Some(':') && use_path.chars().nth(2) == Some('\\') {
            // 已是绝对路径（如 "D:\path"），直接使用
            std::path::PathBuf::from(&use_path)
        } else {
            // 相对路径，拼到共享目录
            dir_path.join(&use_path)
        };
        if !is_path_safe(&dir_path, &requested_full) {
            write_log(&format!("[HttpServer] Path traversal blocked: base={:?} request={:?}", dir_path, requested_full));
            send_error(stream, StatusCode::FORBIDDEN, "Forbidden").await?;
            return Ok(());
        }
        requested_full
    };

    if target.is_dir() {
        handle_dir_browse(stream, &target, request_path).await?;
    } else if target.is_file() {
        handle_file_download(stream, &target, headers).await?;
    } else {
        send_error(stream, StatusCode::NOT_FOUND, "Not Found").await?;
    }

    Ok(())
}

/// 处理目录浏览
async fn handle_dir_browse(
    stream: &mut TcpStream,
    dir_path: &PathBuf,
    display_path: &str,
) -> Result<()> {
    let entries = match std::fs::read_dir(dir_path) {
        Ok(iter) => iter,
        Err(e) => {
            write_log(&format!("[HttpServer] Failed to read dir {:?}: {}", dir_path, e));
            send_error(stream, StatusCode::INTERNAL_SERVER_ERROR, "Cannot read directory").await?;
            return Ok(());
        }
    };

    let mut file_entries = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = path.is_dir();
        let size = if is_dir { 0 } else { entry.metadata().map(|m| m.len()).unwrap_or(0) };
        let modified = entry.metadata().ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        let rel_path = if display_path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", display_path, name)
        };

        file_entries.push(FileEntry {
            name,
            path: rel_path,
            is_dir,
            size,
            modified,
        });
    }

    // 排序：目录优先，名称升序
    file_entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    let body = serde_json::to_string_pretty(&file_entries)?;
    write_response(stream, StatusCode::OK, "application/json; charset=utf-8", body.as_bytes(), false, None).await
}

/// 处理文件下载（支持 Range）
async fn handle_file_download(
    stream: &mut TcpStream,
    file_path: &PathBuf,
    headers: &HeaderMap,
) -> Result<()> {
    let file_size = match std::fs::metadata(file_path) {
        Ok(m) => m.len(),
        Err(_) => {
            send_error(stream, StatusCode::NOT_FOUND, "File not found").await?;
            return Ok(());
        }
    };

    let filename = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    let mime = MimeGuess::from_path(file_path)
        .first_or_octet_stream()
        .to_string();

    // 解析 Range header
    let range_opt = headers.get("range").and_then(|v| v.to_str().ok()).and_then(|s| parse_range(s, file_size));
    let if_range = headers.get("if-range").and_then(|v| v.to_str().ok());

    // 检查 If-Range
    if let Some(ref range) = range_opt {
        if let Some(if_range_val) = if_range {
            // 简化处理：If-Range 条件不满足则返回 200
            // (完整的 ETag/Last-Modified 对比略过)
            if if_range_val != "*" {
                // 假设有效，继续处理
                let _ = if_range_val;
            }
        }
    }

    match range_opt {
        Some(range) => {
            // 206 Partial Content
            let start = range.start;
            let end = range.end.unwrap_or(file_size.saturating_sub(1));
            let end = std::cmp::min(end, file_size.saturating_sub(1));

            if start > end || start >= file_size {
                // Invalid range
                let body = "Requested Range Not Satisfiable";
                let resp = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\n\
                    Content-Type: text/plain\r\n\
                    Content-Range: bytes */{}\r\n\
                    Content-Length: {}\r\n\
                    Connection: close\r\n\
                    \r\n",
                    file_size,
                    body.len()
                );
                stream.write_all(resp.as_bytes()).await?;
                stream.write_all(body.as_bytes()).await?;
                return Ok(());
            }

            let content_length = end - start + 1;

            // 构建 206 响应头
            let header_str = format!(
                "HTTP/1.1 206 Partial Content\r\n\
                Content-Type: {}\r\n\
                Content-Length: {}\r\n\
                Content-Range: bytes {}-{}/{}\r\n\
                Content-Disposition: attachment; filename=\"{}\"\r\n\
                Accept-Ranges: bytes\r\n\
                Connection: close\r\n\
                \r\n",
                mime,
                content_length,
                start,
                end,
                file_size,
                filename
            );

            stream.write_all(header_str.as_bytes()).await?;

            // 发送文件内容（Range 部分）
            let mut file = File::open(file_path)?;
            file.seek(SeekFrom::Start(start))?;
            let mut buf = vec![0u8; CHUNK_SIZE];
            let mut remaining = content_length;

            while remaining > 0 {
                let to_read = std::cmp::min(CHUNK_SIZE, remaining as usize);
                let n = file.read(&mut buf[..to_read])?;
                if n == 0 {
                    break;
                }
                stream.write_all(&buf[..n]).await?;
                remaining -= n as u64;
            }
        }
        None => {
            // 200 OK，发送完整文件
            let header_str = format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: {}\r\n\
                Content-Length: {}\r\n\
                Content-Disposition: attachment; filename=\"{}\"\r\n\
                Accept-Ranges: bytes\r\n\
                Connection: close\r\n\
                \r\n",
                mime,
                file_size,
                filename
            );

            stream.write_all(header_str.as_bytes()).await?;

            // 发送文件内容
            let mut file = File::open(file_path)?;
            let mut buf = vec![0u8; CHUNK_SIZE];
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                stream.write_all(&buf[..n]).await?;
            }
        }
    }

    Ok(())
}

/// 解析 Range header，e.g. "bytes=0-1023"
fn parse_range(s: &str, file_size: u64) -> Option<RangeSpec> {
    if !s.starts_with("bytes=") {
        return None;
    }
    let spec = &s[6..];
    let parts: Vec<&str> = spec.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start: u64 = parts[0].parse().ok()?;
    let end: Option<u64> = if parts[1].is_empty() {
        None
    } else {
        Some(parts[1].parse().ok()?)
    };

    Some(RangeSpec { start, end: end.map(|e| std::cmp::min(e, file_size.saturating_sub(1))) })
}

/// 路径安全检查：确保 target 路径在 base 目录内
fn is_path_safe(base: &PathBuf, target: &PathBuf) -> bool {
    // 检查 target 是否在 base 下（通过规范化路径）
    // canonicalize 会跟随符号链接并规范化路径

    let base_canon = match base.canonicalize() {
        Ok(p) => p,
        Err(_) => base.clone(),
    };

    // 对于 target，如果不存在则检查其组成的路径安全性
    let target_canon = match target.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            // 文件不存在：使用 components 检查路径遍历
            let components = target.components().collect::<Vec<_>>();
            let mut depth: isize = 0;

            for comp in &components {
                match comp {
                    std::path::Component::Normal(_) => depth += 1,
                    std::path::Component::ParentDir => depth -= 1,
                    std::path::Component::CurDir => {}
                    _ => return false, // 拒绝 RootDir、Prefix 等
                }
                if depth < 0 {
                    return false; // ../ 超出 base 范围
                }
            }

            // 规范化后必须和 base 做 starts_with 比较
            let normalized = normalize_path(target);
            let base_canon = match base.canonicalize() {
                Ok(p) => p,
                Err(_) => base.clone(),
            };
            return normalized.starts_with(&base_canon);
        }
    };

    // target 必须在 base 目录下
    target_canon.starts_with(&base_canon)
}

/// 规范化路径（解析 .. 和 . 组件）
fn normalize_path(path: &PathBuf) -> PathBuf {
    let mut result = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            _ => result.push(comp),
        }
    }
    result
}

/// 发送 HTTP 响应
async fn write_response(
    stream: &mut TcpStream,
    status: StatusCode,
    content_type: &str,
    body: &[u8],
    is_range: bool,
    range_info: Option<(&str, u64, u64, u64)>, // (header_name, start, end, total)
) -> Result<()> {
    let status_text = status.canonical_reason().unwrap_or("Unknown");

    if is_range {
        if let Some((range_header, start, end, total)) = range_info {
            let header_str = format!(
                "HTTP/1.1 {} {}\r\n\
                Content-Type: {}\r\n\
                Content-Length: {}\r\n\
                Content-Range: bytes {}-{}/{}\r\n\
                Accept-Ranges: bytes\r\n\
                Connection: close\r\n\
                \r\n",
                status.as_u16(),
                status_text,
                content_type,
                end - start + 1,
                start,
                end,
                total
            );
            stream.write_all(header_str.as_bytes()).await?;
        }
    } else {
        let header_str = format!(
            "HTTP/1.1 {} {}\r\n\
            Content-Type: {}\r\n\
            Content-Length: {}\r\n\
            Connection: close\r\n\
            \r\n",
            status.as_u16(),
            status_text,
            content_type,
            body.len()
        );
        stream.write_all(header_str.as_bytes()).await?;
        stream.write_all(body).await?;
    }

    Ok(())
}

/// 发送 HTTP 错误响应
async fn send_error(
    stream: &mut TcpStream,
    status: StatusCode,
    message: &str,
) -> Result<()> {
    let body = serde_json::json!({
        "error": status.as_u16(),
        "message": message
    }).to_string();

    let header_str = format!(
        "HTTP/1.1 {} {}\r\n\
        Content-Type: application/json\r\n\
        Content-Length: {}\r\n\
        Connection: close\r\n\
        \r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Error"),
        body.len()
    );

    stream.write_all(header_str.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    Ok(())
}
