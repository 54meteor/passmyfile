# 隔空投送 - 迭代记录

## 项目信息

- **项目名称**: 隔空投送 (Gekoto Transfer)
- **项目路径**: `D:\aicode\feige-transfer`
- **技术栈**: Tauri 2.x + React + TypeScript + Rust

---

## 第二次迭代 (2026-03-22)

### 迭代目标

修复 MVP 版本遗留问题，优化参数命名，提升代码稳定性。

### 功能调整

#### 1. 参数命名规范化

**问题描述**：
前端调用 `confirm_receive` 命令时出现参数不匹配错误。

**错误信息**：
```
invalid args 'requestId' for command 'confirm_receive': 
command confirm_receive missing required key requestId
```

**排查过程**：
1. Rust 代码参数名：`request_id`
2. TypeScript 调用参数名：`request_id`
3. 打包后的 JS 文件包含正确的 `request_id`
4. 但运行时报错提示缺少 `requestId`

**根本原因**：
Tauri 在 Windows 上的序列化/反序列化过程中，snake_case 和 camelCase 混用导致的不兼容问题。

**解决方案**：
将所有相关参数名统一为 `req_id`，避免 snake_case 和 camelCase 冲突。

**修改文件**：
- `src/App.tsx` - 前端调用
- `src-tauri/src/lib.rs` - Rust 后端命令

---

#### 2. Windows 编译缓存问题

**问题描述**：
代码修改后，Windows 上运行的 exe 仍使用旧版本逻辑。

**原因分析**：
- WSL 环境编译的 Windows exe 可能存在路径或缓存问题
- `target` 目录可能包含旧的编译产物

**解决方案**：
清理编译缓存后重新编译：
```bash
rmdir /s /q src-tauri\target
npm run tauri build
```

---

### 待解决项

1. **Windows exe 重新编译** - 主公需在 Windows 上执行编译
2. **接收确认弹窗功能** - 功能代码已完成，但因编译问题无法测试

### 当前功能状态

| 功能 | 状态 | 说明 |
|------|------|------|
| 局域网设备发现 | ✅ | 原计划 UDP 广播，因网络限制改用手动IP连接 |
| 手动IP连接 | ✅ | 用户输入对方IP进行TCP连接 |
| 文件传输 | ✅ | TCP直连，64KB分片传输 |
| 传输进度显示 | ⏳ | 待开发 |
| 接收确认弹窗 | ⏳ | 代码完成，需测试验证 |

### 技术债务

1. **UDP广播自动发现** - 当前使用手动IP连接，未来可在支持广播的网络环境启用
2. **传输进度UI** - 后端已支持进度计算，前端展示待开发
3. **断点续传** - 尚未实现

---

## 第一次迭代成果 (2026-03-21)

### 完成内容

1. **项目初始化** - Tauri 2.x + React + TypeScript + Rust
2. **核心功能开发**
   - 本机IP显示
   - 手动IP连接（替代UDP广播）
   - TCP文件传输
   - 系统文件对话框集成
3. **问题解决**
   - Rust版本升级 (1.75 → 1.94)
   - Tokio Runtime被drop导致服务终止
   - 死锁问题（无限循环持有锁）
   - HTML input无法获取完整文件路径
4. **项目改名**: 飞鸽传书 → 隔空投送
5. **文档编写**
   - `docs/DEVELOPMENT.md` - 开发文档
   - `docs/WECHAT-ARTICLE.md` - 公众号文章素材

---

## 网络信息（测试用）

- **电脑A**: 192.168.31.11
- **电脑B**: 192.168.31.9
- **传输端口**: 18766

---

*最后更新: 2026-03-23*
