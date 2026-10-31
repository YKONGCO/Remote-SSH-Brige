# 开发指南

## 环境与启动

开发命令直接使用系统 PATH 中的 Node.js、Cargo 和本地 npm 依赖。Node.js 支持范围见 `package.json` 的 `engines`，推荐 24.15+ 的 24.x 版本；Rust 使用 stable。Windows 还需 Microsoft C++ Build Tools、WebView2 Runtime 和 Perl，推荐 Rust MSVC 工具链。其他平台的桌面构建与打包尚未验证。

```sh
npm ci
npm run desktop
```

`npm run dev` 启动浏览器演示，`npm run desktop` 通过 Tauri 启动桌面应用，`npm run desktop:build` 构建发行程序与 NSIS 安装包。

## 验证

```sh
# TypeScript 检查与前端生产构建
npm run build

# React 界面测试
npm run test:ui

# Rust 后端测试，不编译桌面特性
npm test

# 桌面特性编译检查
cargo check --manifest-path src-tauri/Cargo.toml

# Rust 格式检查
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

协议测试先构建无窗口服务：

```sh
npm run build:headless
npm run test:e2e
```

测试在回环地址创建一次性 SSH / SFTP fixture，使用真实 MCP JS 客户端连接 Rust 服务，不访问用户服务器。远程命令和文件系统由内存服务实现，不能代替真实 Linux / OpenSSH 环境验收。输出位于 `.artifacts/integration-*/report.json`；程序路径识别平台可执行文件后缀并支持 `CARGO_TARGET_DIR`，也可用 `REMOTE_DESK_TEST_BINARY` 直接指定。

## 代码结构

| 路径                                             | 职责                                     |
| ------------------------------------------------ | ---------------------------------------- |
| `src/App.tsx`                                    | 工作区与页面导航                         |
| `src/components/`                                | 服务器、文件、终端、审批、审计与设置界面 |
| `src/components/ui/`                             | shadcn/ui 基础组件                       |
| `src/api.ts`                                     | Tauri IPC 与浏览器演示                   |
| `src/theme-tokens.css`                           | 亮暗主题与语义颜色                       |
| `src/styles.css`、`src/fluent.css`               | 布局与组件样式                           |
| `src-tauri/src/model.rs`                         | 配置、设置与操作数据类型                 |
| `src-tauri/src/database.rs`、`schema.sql`        | SQLite 持久化与当前表结构                |
| `src-tauri/src/state.rs`                         | 共享状态、权限校验、执行入口和审计       |
| `src-tauri/src/mcp.rs`、`mcp_args.rs`            | MCP 服务、工具路由与参数 Schema          |
| `src-tauri/src/clients.rs`                       | 客户端配置检测、备份和写入               |
| `src-tauri/src/ssh.rs`、`terminal.rs`            | SSH / SFTP 与 PTY 会话                   |
| `src-tauri/src/file_tools.rs`、`safe_remove.rs`  | 文本读写编辑与删除预检                   |
| `src-tauri/src/security.rs`、`command_policy.rs` | 只读保护、审批与命令策略                 |
| `src-tauri/src/lib.rs`                           | 桌面命令、窗口和托盘入口                 |
| `src-tauri/tests/`、`scripts/integration.mjs`    | 后端与协议测试                           |

桌面 IPC 和 MCP 请求共用后端操作入口，依次校验服务器状态、权限和安全策略，再执行 SSH 操作并记录审计。浏览器演示与真实工作区隔离。

表结构集中保存在 `schema.sql`，应用初始化和协议测试共用，当前版本为 SQLite schema v2。修改数据库格式时应明确版本边界，避免静默创建空工作区或改变已有权限。

## 贡献约定

提交改动时说明解决的问题、行为变化和验证方法。安全策略、工具参数或文件提交逻辑发生变化时，补充相应的拒绝路径或边界测试，并更新用户文档。

界面代码使用 TypeScript 和 Prettier，Rust 使用 rustfmt。可运行 `npm run format` 格式化前端与文档。不要提交 `.artifacts/`、`.tools/`、`dist/`、`node_modules/`、Rust 构建输出、凭据或真实服务器配置。

主题和组件使用现有语义变量；新增第三方代码或资源时保留许可证，并更新 [第三方声明](THIRD_PARTY_NOTICES.md)。
