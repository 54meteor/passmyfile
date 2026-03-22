# 从零开发一个"隔空投送"：一个程序员与AI的协作记录

## 前言

今天，一个朋友问我："你知道隔空投送这个软件吗？能不能自己开发一个？"

说实话，这个需求让我眼前一亮。隔空投送 IPMsg 是一个经典的局域网通讯工具，但已经很多年没有更新了。如果能用现代技术重新实现，功能更强、体验更好，岂不是很有意思？

于是，一场程序员与AI的协作开发开始了。

---

## 一、需求确认

朋友的需求很明确：

1. **核心功能**：桌面客户端，多台电脑之间方便传输文件
2. **跨平台**：Windows、macOS、Ubuntu、Android、iOS
3. **简单实用**：不需要复杂的企业功能

我的建议是用 **Tauri** 技术栈——Rust 后端 + Web 前端，包体积小、性能好、一套代码多平台运行。

**开发周期预估**：10-12天 MVP

---

## 二、技术方案

### 2.1 网络架构

最初的设计是 UDP 广播发现 + TCP 文件传输：

```
发现协议 (UDP):
  - 端口: 18765
  - 每5秒广播一次自身信息

传输协议 (TCP):
  - 端口: 18766
  - 发送文件元信息 → 接收确认 → 传输数据
```

### 2.2 项目结构

```
gekoto-transfer/
├── src/              # React 前端
├── src-tauri/        # Rust 后端
│   ├── discovery.rs  # 设备发现模块
│   ├── transfer.rs   # 文件传输模块
│   └── lib.rs        # 核心逻辑
└── docs/             # 文档
```

---

## 三、踩坑记录

### 3.1 第一个坑：Rust 版本过旧

项目初始化时，编译报错：

```
error: package `time-core v0.1.8` requires the Cargo feature called `edition2024`
```

**原因**：Rust 版本 1.75.0 太旧了

**解决**：升级到 1.94.0
```bash
rustup update stable
```

---

### 3.2 第二个坑：UDP 广播被路由器阻止

程序运行后，两台电脑互相发现不了。

**排查过程**：
1. 确认两台电脑在同一局域网（192.168.31.x）✓
2. Python 测试 UDP 单播正常 ✓
3. UDP 广播被路由器阻断了 ✗

**解决**：改用手动 IP 连接方式。用户输入对方 IP，程序尝试 TCP 连接，连接成功就说明对方在线。

> 教训：UDP 广播在大多数家庭/办公网络中不可靠，TCP 单播更稳定。

---

### 3.3 第三个坑：Tokio Runtime 被 drop

程序启动后，TCP 端口没有监听，程序似乎"假死"了。

**原因**：`setup()` 函数中的 `block_on()` 完成后，runtime 立即被 drop，所有异步任务随之终止。

**解决**：在新线程中运行 runtime，并进入无限循环保持其活跃：

```rust
std::thread::spawn(|| {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // 初始化...
        
        // 保持 runtime 运行
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
    });
});
```

---

### 3.4 第四个坑：死锁

程序启动后，UI 完全无响应，点击按钮没反应。

**原因**：无限循环持有了 `manager` 的锁，导致其他命令无法获取锁。

**解决**：确保初始化完成后立即释放锁：

```rust
let init_result = {
    let mut manager = get_manager().lock().await;
    manager.init().await
}; // 锁在这里释放

// 然后才进入无限循环
loop {
    tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
}
```

> 教训：在 async 环境中，锁的生命周期管理非常重要。

---

### 3.5 第五个坑：文件路径获取

文件传输功能测试时，提示"系统找不到指定的文件"。

**原因**：HTML 的 `<input type="file">` 无法获取完整文件路径。

**解决**：使用 Tauri 原生文件对话框：

```rust
// Rust 端
tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())

// React 前端
import { open } from '@tauri-apps/plugin-dialog';
const filePath = await open({
    multiple: false,
    directory: false,
    title: '选择要发送的文件'
});
```

---

## 四、开发成果

经过一天的协作开发，MVP 版本基本完成：

### 功能清单

| 功能 | 状态 |
|------|------|
| 显示本机 IP 地址 | ✅ |
| 手动添加连接 | ✅ |
| TCP 文件传输 | ✅ |
| 系统文件对话框 | ✅ |

### 使用方法

1. 运行程序，界面显示本机 IP（如 `192.168.31.11:18766`）
2. 在另一台电脑上输入这个 IP，点击"添加"
3. 连接成功后，点击"选择文件发送"

---

## 五、经验总结

### 5.1 技术层面

1. **网络编程**：UDP 广播在真实网络环境中往往不可靠，TCP 单播更实用
2. **Tokio 异步**：runtime 需要显式保持运行，注意锁的生命周期
3. **Tauri 开发**：原生插件需要声明权限，文件操作用对话框更可靠

### 5.2 协作层面

1. **版本同步**：两端必须运行相同版本的程序，否则可能不兼容
2. **日志调试**：将日志写入文件，比 console.log 更可靠
3. **渐进式开发**：先跑通核心功能，再迭代优化细节

### 5.3 AI 协作

这次开发完全是云端协作：
- 用户测试并反馈问题
- 我分析问题、修改代码
- 用户验证修复

效率比传统开发模式高很多。

---

## 六、后续计划

MVP 完成，但还有很多可以改进的地方：

- [ ] UDP 广播自动发现（针对支持广播的网络）
- [ ] 传输进度条
- [ ] 文件接收确认弹窗
- [ ] 传输历史记录
- [ ] 跨平台打包（macOS、Linux、iOS、Android）

---

## 结语

从一个简单的想法开始，到一个可用的 MVP，只用了一天时间。这得益于：

1. **Tauri** 成熟的生态
2. **Rust** 高效的异步编程
3. **云端协作** 的便利

如果你也有类似的想法，欢迎尝试！

**项目地址**：`D:\aicode\gekoto-transfer`

---

*开发日期：2026-03-21*
*作者：子龙 & ChatGPT*
