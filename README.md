# Remote SSH MCP

通过 MCP 将 SSH 服务器的命令执行、文件操作和持久终端提供给 AI Agent，并在桌面端统一管理连接、权限、审批和审计。

基于 Tauri 2、React、TypeScript 和 Rust 构建，使用 SQLite 保存配置，通过 SSH / SFTP 访问远程服务器。当前主要支持 Windows，其他平台的构建与打包尚未验证。

## 功能

- **服务器管理**：多服务器、分组与搜索，支持密码、私钥和 SSH Agent 认证，以及 SHA-256 主机指纹核验。
- **MCP 接入**：Streamable HTTP、Bearer Token，提供 8 个工具，支持 Claude Code、Codex、Gemini CLI、OpenCode、Cursor 和 Pi 一键配置。
- **文件操作**：SFTP 浏览与传输、文本编辑、图片预览；Agent 可分页读取、批量编辑和按授权范围安全删除。
- **持久终端**：桌面与 MCP 共用 SSH PTY 会话，保留工作目录和交互进程状态。
- **权限与审批**：逐项授权、只读保护、命令策略，以及敏感操作的单次人工审批。
- **本地审计**：保存操作结果与有界输出预览，支持筛选、查看详情和 JSON 导出。

## 快速开始

### 环境准备

- Node.js **24.15 或更高的 24.x 版本**，以及 npm。
- Rust stable；Windows 推荐使用 MSVC 工具链。
- Windows 开发环境：Microsoft C++ Build Tools、WebView2 Runtime，以及构建内置 OpenSSL 所需的 Perl。

### 从源码运行

获取源码后，在项目根目录执行：

```sh
npm ci
npm run desktop
```

仅预览界面：

```sh
npm run dev
```

打开 `http://127.0.0.1:1420`。浏览器预览使用模拟数据；真实 SSH、系统凭据库和 MCP 服务在桌面端运行。

### 连接服务器与 Agent

1. 添加 SSH 服务器，填写地址、账户和认证信息。私钥路径使用本地绝对路径。
2. 核对服务器的 SHA-256 主机指纹后信任，并测试连接。
3. 在服务器编辑窗口授予所需 MCP 权限；新服务器默认仅允许读取和预览。
4. 打开「MCP 服务」页并启动服务，选择客户端进行一键配置，或复制手动配置。
5. 在客户端重新连接 MCP。服务每次启动都会更换 Token，需要同步更新客户端配置。

默认端点为 `http://127.0.0.1:18920/mcp`，配置键为 `remote-ssh`。完整设置见 [MCP 客户端接入](docs/MCP_CLIENTS.md)。

## MCP 工具

| 工具           | 用途                                       |
| -------------- | ------------------------------------------ |
| `list_servers` | 发现已授权服务器、权限和安全策略           |
| `exec`         | 执行命令，返回标准输出、错误输出和退出码   |
| `read_file`    | 按行分页读取 UTF-8 文本                    |
| `write_file`   | 创建或完整覆盖文本文件                     |
| `edit_file`    | 批量匹配并替换文本，返回变更差异           |
| `remove_path`  | 删除授权根目录内的文件、链接或目录         |
| `preview_file` | 只读预览文本和图片                         |
| `terminal`     | 列出、打开、读取、输入、调整尺寸和关闭终端 |

先调用 `list_servers` 获取服务器 ID 和权限，再调用相应工具。参数、示例和限制见 [MCP 工具参考](docs/MCP_TOOLS.md)。

## 安全边界

文件根目录约束 SFTP 操作。**`exec` 和终端使用 SSH 账户的 Shell 权限，不受文件根目录限制。** 命令策略属于静态操作保护，无法完整检查外部脚本、任意解释器代码或交互终端输入。

MCP 默认仅监听本机；远程接入需显式修改监听地址。跨不可信网络应使用 VPN 或 HTTPS 代理。凭据保存在系统凭据库，命令文本及其输出可能进入审计，请按实际需要授权。详见 [安全控制](docs/SECURITY_CONTROLS.md)。

## 构建与验证

```sh
# 生成桌面程序和安装包
npm run desktop:build

# 前端构建、界面测试和后端测试
npm run build
npm run test:ui
npm test
```

默认 Windows 程序位于 `src-tauri/target/release/remote-desk.exe`，安装包位于 `src-tauri/target/release/bundle/`。协议测试、无窗口服务和项目结构见 [开发指南](docs/DEVELOPMENT.md)。

## 文档

- [使用指南](docs/USAGE.md)：文件、终端、托盘行为、设置和数据备份。
- [MCP 客户端接入](docs/MCP_CLIENTS.md)：一键配置、手动配置与远程接入。
- [MCP 工具参考](docs/MCP_TOOLS.md)：参数、权限、返回结果与错误边界。
- [安全控制](docs/SECURITY_CONTROLS.md)：命令策略、只读保护、审批与审计。
- [开发指南](docs/DEVELOPMENT.md)：构建、测试、代码结构和贡献约定。

## 贡献

欢迎提交 Issue 和 Pull Request。报告问题时请附上复现步骤、系统环境和相关错误信息，并移除服务器凭据、Token 及其他敏感数据。提交代码前请运行与改动相关的检查，更新受影响的文档。

## 许可证

本项目采用 [MIT License](LICENSE)。
