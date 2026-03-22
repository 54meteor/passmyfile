# 隔空投送 - 开发文档

## 项目概述

**项目名称**: 隔空投送 (隔空投送)
**项目定位**: 跨平台局域网文件传输工具
**技术栈**: Tauri 2.x + React + TypeScript + Rust
**项目路径**: `D:\aicode\gekoto-transfer`

---

## 一、需求分析

### 1.1 用户需求

用户希望开发一个类似"隔空投送"的局域网文件传输工具，具备以下特点：
- 跨平台：Windows、macOS、Linux、Android、iOS
- 轻量级：不需要复杂的企业功能
- 核心功能：多台电脑之间方便传输文件

### 1.2 技术选型

| 方案 | 语言 | 优点 | 缺点 |
|------|------|------|------|
| Qt | C++/Go | 跨平台、性能好 | 开发难度高 |
| Electron | JavaScript | Web技术栈、开发快 | 包体积大、功耗高 |
| Tauri | Rust + Web | 包体积小、性能好、生态成熟 | 移动端支持中 |

**最终选择**: Tauri 2.x + React + TypeScript + Rust

---

## 二、开发过程

### 2.1 项目初始化

**日期**: 2026-03-21
**操作**:
```bash
npm create tauri-app@latest gekoto-transfer -- --template react-ts --manager npm
```

**遇到问题**:
1. Rust 版本过旧 (1.75.0)，需要升级到 1.94.0
2. Linux 环境缺少 GTK 开发库

**解决方案**:
- 使用 `rustup update stable` 升级 Rust
- 安装 `libgtk-3-dev libwebkit2gtk-4.1-dev` 等依赖

### 2.2 网络架构设计

**初始方案**: UDP 广播发现 + TCP 文件传输

```
发现协议 (UDP):
  - 端口: 18765
  - 方式: 每5秒广播一次自身信息

传输协议 (TCP):
  - 端口: 18766
  - 连接: 发起方主动连接接收方
```

### 2.3 核心问题与解决

#### 问题一：UDP 广播被路由器阻止

**现象**: 设备之间无法发现对方

**排查过程**:
1. 确认两台电脑在同一局域网 (192.168.31.x)
2. 使用 Python 测试 UDP 单播通信正常
3. 发现路由器阻断了 UDP 广播包

**解决方案**: 改用手动 IP 连接方式
- 用户输入对方 IP 地址
- 程序尝试 TCP 连接
- 如果连接成功，说明对方在线

#### 问题二：Tokio Runtime 被 drop

**现象**: 程序启动后 TCP 服务立即终止

**原因分析**:
- `setup()` 函数中的 `block_on()` 完成后
- runtime 被 drop
- 导致所有异步任务被终止

**解决方案**:
```rust
// 在单独的线程中运行 tokio runtime
std::thread::spawn(|| {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // 初始化...
        
        // 初始化完成后进入无限循环保持 runtime 活跃
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
    });
});
```

#### 问题三：死锁问题

**现象**: 程序启动后 UI 无响应

**原因分析**:
- 无限循环持有 `manager` 的锁
- 导致其他命令无法获取锁

**解决方案**:
```rust
rt.block_on(async {
    // 先初始化（使用独立代码块确保锁释放）
    let init_result = {
        let mut manager = get_manager().lock().await;
        manager.init().await
    };
    
    // 初始化完成后，锁被释放
    // 然后进入无限循环
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
});
```

#### 问题四：文件路径获取

**现象**: `Cannot find file` 错误

**原因**: HTML `<input type="file">` 无法获取完整文件路径

**解决方案**: 使用 Tauri 原生文件对话框
```rust
// 安装插件
npm install @tauri-apps/plugin-dialog

// 前端代码
import { open } from '@tauri-apps/plugin-dialog';
const filePath = await open({
    multiple: false,
    directory: false,
    title: '选择要发送的文件'
});
```

---

## 三、关键技术点

### 3.1 Tauri 2.x 插件系统

需要正确配置 capabilities：
```json
// capabilities/default.json
{
  "permissions": [
    "core:default",
    "opener:default",
    "dialog:default"
  ]
}
```

### 3.2 Tokio 异步编程

- 使用 `tokio::net::TcpListener` 进行异步 TCP 监听
- 使用 `std::thread::spawn` 在独立线程运行 runtime
- 注意锁的生命周期，避免死锁

### 3.3 文件传输协议

```rust
// 元信息格式
"FILE:{文件名}:{文件大小}"

// 传输流程
1. 发送方连接接收方 (TCP)
2. 发送 "FILE:xxx:size" 元信息
3. 接收方回复 "ACK"
4. 开始传输文件数据 (64KB 分片)
```

---

## 四、当前功能状态

| 功能 | 状态 | 说明 |
|------|------|------|
| 显示本机IP | ✅ 完成 | 界面右上角显示 IP:端口 |
| 手动添加连接 | ✅ 完成 | 用户输入对方IP，TCP连接检测 |
| 文件传输 | ✅ 完成 | TCP直连，64KB分片传输 |
| 系统文件对话框 | ✅ 完成 | 使用Tauri原生对话框 |
| 传输进度显示 | ⏳ 待开发 | - |
| 接收确认弹窗 | ⏳ 待开发 | - |

---

## 五、已知问题和限制

1. **设备发现**: 暂时使用手动IP连接，不支持自动发现
2. **传输进度**: 暂未显示传输速度和百分比
3. **断点续传**: 暂不支持中断后续传
4. **跨平台**: 仅测试了 Windows 版本

---

## 六、未来改进方向

1. **UDP广播自动发现** - 如果网络环境支持广播
2. **传输进度条** - 实时显示传输速度和进度
3. **文件接收确认** - 弹出对话框询问是否接收
4. **传输历史记录** - 记录传输历史
5. **跨平台打包** - macOS、Linux、iOS、Android

---

## 七、经验总结

### 7.1 网络编程要点

1. **Windows 防火墙** 会阻止大多数 UDP 广播
2. **TCP 单播** 比 UDP 广播更可靠
3. ** Tokio runtime** 需要显式保持运行

### 7.2 Tauri 开发要点

1. **插件需要声明权限** - 在 capabilities 中配置
2. **文件操作** 使用原生对话框而非 HTML input
3. **异步初始化** 需要注意锁的生命周期

### 7.3 开发协作要点

1. **频繁同步代码** - 使用 Git 或共享文件夹
2. **日志调试** - 写入文件便于排查问题
3. **版本一致性** - 确保两端运行相同版本程序

---

**文档编写时间**: 2026-03-21
**作者**: 子龙 (Claude)
