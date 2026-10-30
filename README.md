# SSH MCP Bridge

连接 AI Agent 与 SSH 服务器的 MCP 桥接工具。通过 MCP 将远程服务器的命令执行、文件操作和持久终端提供给 Agent，使用逐项授权、命令策略、人工审批和审计记录控制访问。桌面界面用于配置 SSH 连接、授予权限、处理审批与查看操作结果。

技术栈：Tauri 2 + React / TypeScript + shadcn/ui + Rust MCP SDK（rmcp 3.8.0）+ SQLite + SSH / SFTP。

产品名称为「SSH MCP Bridge」，突出通过 MCP 为 Agent 提供 SSH 远程控制能力。应用标识、现有数据库与 MCP 客户端中的 `remote-desk` 配置键继续沿用，保留已保存的服务器、凭据与客户端连接。下文旧版构建包的文件名为历史产物名称。

Agent 通过 MCP 发现已授权服务器并调用工具；用户可在桌面端调整权限、审批敏感操作或查看审计，也可通过文件和终端页面辅助排查。MCP 服务支持本机及按 IP 远程接入。

## 已实现

- 多服务器新增、编辑、删除、分组、搜索、停用和连接测试。
- 密码、私钥和 SSH Agent 认证；密码和私钥口令存入操作系统凭据库。私钥通过本地绝对路径读取。
- SSH SHA-256 主机指纹核验和固定；主机地址或端口变更后清除旧信任。
- 命令执行：stdout / stderr / exit code、1–300 秒超时、输出容量限制。
- SFTP：目录浏览、UTF-8 文本读取与编辑、文件创建、目录管理、重命名、单文件上传与下载。
- 文件只读预览：文本及 PNG、JPEG、GIF、WebP 图片，支持单独进入编辑或下载。
- [安全控制与审批](docs/SECURITY_CONTROLS.md)：删除保护、诊断命令白名单、只读模式和单次审批；审批默认关闭。
- 持续连接的 SSH PTY：xterm 终端、输入、尺寸调整、增量读取、会话列表与关闭。GUI 和 MCP 访问同一个会话。
- 本机 MCP Streamable HTTP：启动 / 停止、Bearer Token、客户端配置、每服务器独立的执行 / 写入 / 终端权限。
- 统一审计：桌面操作、MCP 调用、失败记录、筛选、详情与 JSON 导出。配置和审计跨应用重启保存。

## 直接运行桌面程序

当前工作区最新界面构建位于 .artifacts/headers/RemoteDesk.exe，须与同目录的 WebView2Loader.dll 放在一起。程序内嵌前端资源，运行时不需要 Node.js 或 Vite。机器上需有 Microsoft WebView2 Runtime。

这是供当前工作区验收的开发构建，已去除调试符号，尚未制作或签名安装包。

## 在当前 Windows 工作区运行

```powershell
npm.cmd install
npm.cmd run desktop
# 等价：npm run tauri dev
```

当前工作区已在被 git 忽略的 .tools 中准备本地 Rust / GNU 编译工具；npm run tauri dev 和 npm run desktop 均通过启动器自动设置当前命令的工具链路径，不修改系统 PATH。首次桌面启动是空工作空间。

浏览器界面预览：

```powershell
npm.cmd run dev
# 打开 http://127.0.0.1:1420
```

**浏览器预览使用明确标记的模拟数据。** 可以操作示例服务器和文件体验界面；真实 SSH、系统凭据和 MCP 服务仅在桌面端运行。演示数据不写入桌面工作空间。

在其他 Windows 机器上开发，请先安装 Node.js、Rust、Microsoft C++ Build Tools 和 WebView2，以及构建内置 OpenSSL 所需的 Perl。推荐 Rust 的 MSVC 工具链。安装依赖后运行 npm.cmd run tauri dev。项目当前主要面向 Windows，其他系统的打包尚未验证。

## 数据迁移与备份

首次启动会将已有 servers.json、audit.previous.jsonl、audit.jsonl 自动导入 SQLite，服务器 ID、主机指纹、权限、旧记录和已保存的返回预览保持不变。密码和私钥口令继续使用系统凭据库，不写入数据库。

迁移在一个事务中完成，遇到损坏数据会明确报错并回滚。原 JSON / JSONL 文件保留且不再更新；导入成功后不会重复导入，也不会恢复已删除的服务器。请从托盘退出旧版，再启动新版，避免旧版继续写入已停用的文件。

备份时先从托盘退出应用，再复制 remote-desk.sqlite3。运行期间可能同时存在 SQLite 的 -wal 和 -shm 文件，请勿只复制正在使用的数据库主文件。登录凭据由系统凭据库单独管理，不包含在数据库备份中。

## 第一次连接

1. 添加服务器，填写主机、端口、账户和认证方式。私钥路径使用绝对路径，勿填写尚未展开的 ~。
2. 保存后，点击服务器菜单的「核验主机指纹」，或点击「测试连接」。
3. 通过服务器控制台 / 管理员核对显示的 SHA-256 指纹，确认后信任。
4. 再次测试连接，即可浏览文件、执行命令与创建终端。
5. 在编辑界面明确开启所需的 MCP 权限。默认仅允许读取文件。
6. 在「MCP 服务」页选择端口并启动，复制生成的配置到支持 Streamable HTTP 和 Authorization 请求头的 MCP 客户端。

应用采用单实例模式。再次打开新版程序时会恢复已有窗口，新进程自动退出；从不同目录打开也使用相同应用标识。旧版尚未包含单实例保护，升级前请先从托盘退出旧版。

每次启动 MCP 都会生成新的 Token；重新启动后需要更新客户端配置。默认监听 127.0.0.1。在「设置 → 常规 → 关闭窗口时」可选择「最小化到托盘」或「直接退出」，默认收起到托盘，MCP 服务与终端保持运行。选择直接退出后，关闭按钮与 Alt+F4 会结束应用和后台服务；托盘右键菜单中的「退出应用」始终直接退出。标题栏单独的收起托盘按钮始终隐藏窗口，点击托盘图标可恢复。

### 从其他主机调用 MCP

1. 停止 MCP 服务，将「监听 IP」设为本机局域网 IP（例如 `192.168.1.10`），或使用 `0.0.0.0` 监听全部 IPv4 网卡，然后启动服务。也支持 IPv6 监听地址。
2. 在主机防火墙中放行所选 TCP 端口（默认 `18920`）。
3. 如果监听全部网卡，在「客户端连接 IP」中填写其他主机能访问的本机 IP。复制生成的配置到另一台主机的 MCP 客户端，地址形如 `http://192.168.1.10:18920/mcp`，请求携带 `Authorization: Bearer <Token>`。

`0.0.0.0` 和 `::` 仅用于监听，不能作为远程客户端的目标 IP。本机客户端的一键配置会自动使用合适的本机地址。仅支持 IP 直连，浏览器 Origin 请求仍被拒绝。HTTP 适用于可信局域网；跨公网请使用 VPN 或 HTTPS 反向代理，代理需要保留受支持的 IP Host。无界面服务也支持 `remote-desk-headless <data-directory> [port] [listen-ip]`，省略监听 IP 时仍只开放本机。

## MCP 工具

| 工具              | 作用 / 主要参数                                                                    |
| ----------------- | ---------------------------------------------------------------------------------- |
| list_servers      | 启用的服务器、ID、文件根目录和权限，不返回凭据                                     |
| exec              | server_id、command、timeout_secs；返回 stdout、stderr、exit_code、truncated        |
| list_files        | server_id、path、offset?（从 0 开始）、limit?（默认 200，最多 1000）；目录分页     |
| read_file         | server_id、path、offset?（从 1 开始）、limit?（默认及最多 2000）；最多 50 KiB 文本 |
| write_file        | server_id、path、content、mode?；默认 overwrite，create 仅创建，父目录须存在       |
| edit_file         | server_id、path、edits[{old_text,new_text}]；匹配原文，批量提交，返回有界 diff     |
| preview_file      | server_id、path；返回只读文本或图片预览，最大 2 MiB 且受下载上限约束               |
| mkdir / remove    | server_id、path；remove 只删除文件或空目录                                         |
| rename            | server_id、path、target；不覆盖已有目标                                            |
| upload / download | server_id、path；传输 base64 content / 返回 base64                                 |
| list_terminals    | 列出 MCP 可访问的终端会话                                                          |
| terminal_open     | server_id、cols、rows；返回 session_id                                             |
| terminal_read     | server_id、session_id、cursor；返回 base64、下一 cursor、alive、truncated          |
| terminal_write    | server_id、session_id、content；执行命令需要包含换行                               |
| terminal_resize   | server_id、session_id、cols、rows                                                  |
| terminal_close    | server_id、session_id                                                              |

共 18 个工具，每个工具使用独立参数 Schema，拒绝无关字段。先 list_servers 获取 ID、权限与安全策略，再调用相应工具。文件路径使用远端 POSIX 绝对路径。

0.2.0 的 `read_file` 返回原始文本及续读提示，`structuredContent` 返回 `has_more`、`next_offset`、行号范围等元数据；客户端不能再把文本块作为旧版 JSON 全文解析。行数或字节限制先达到者生效，续读传 `next_offset`。超长单行不返回部分行，提示使用 `download`。完整字节传输继续使用 `download`，桌面编辑器仍读取完整原文。升级后需重新连接并发现工具。

`edit_file` 同时需要 `read_file` 和 `edit_file` 权限，旧配置不会自动获得编辑权限。每项 `old_text` 必须在同一份原文中唯一匹配，各项不得重叠；任一项失败均不提交。先精确匹配，失败后进行有限的 Unicode/空白归一化匹配；写回保留 BOM 和检测到的换行样式，混合换行可能统一。正文及 diff 不写入审计。

```json
{
  "server_id": "服务器 ID",
  "path": "/srv/app/config.json",
  "edits": [{ "old_text": "\"port\": 8080", "new_text": "\"port\": 9090" }]
}
```

写入、编辑和上传通过同目录临时文件提交：覆盖依赖 `posix-rename@openssh.com`，仅创建依赖 `hardlink@openssh.com`，服务端不支持则明确失败。提交期间断连返回 `COMMIT_STATUS_UNKNOWN`，应先读回核验再决定是否重试。同一服务器配置的桌面与 MCP 文件修改串行化；这不锁住远端其他进程。完整接口与边界见 [文件工具设计](docs/MCP_FILE_TOOLS_DESIGN.md)。

```text
terminal_open(server_id="...", cols=100, rows=30) → session_id
terminal_write(server_id="...", session_id="...", content="cd /srv/app\n")
terminal_write(server_id="...", session_id="...", content="pwd\n")
terminal_read(server_id="...", session_id="...", cursor=0)
# 后续读取传上次返回的 cursor；返回 base64 字节，需解码显示。
```

## 权限、数据与边界

- 凭据存储：系统凭据库服务名 dev.remotedesk.ssh，条目名为服务器 ID。
- 配置、业务设置和审计：统一保存在 Tauri app_data_dir 下的 remote-desk.sqlite3。SQLite 使用内置引擎，无需另装数据库服务。
- 日志保存操作类型、目标路径或命令、来源、结果与耗时，以及有界返回预览。命令标准输出和错误输出各保留最多 16 KiB；其他返回预览最多 64 KiB，截断会明确标注。凭据、直接读取或下载的文件正文和终端输入不写入日志；命令文本及其打印内容会记录。
- 审计历史持续保存在 SQLite；列表显示最近 500 条，详情按记录 ID 读取。旧版 10 MB JSONL 滚动归档机制已停用。
- 新配置的上传与下载上限默认各为 128 MiB，可在设置中分别输入 1–256 MiB 的自定义整数，按回车或离开输入框保存。已有保存值保留不变；限制同时约束文本写入与读取，桌面和 MCP 共用后端校验。MCP 请求体限额随当前配置生效，无需重启服务。当前为整文件 Base64 传输，上限控制在 256 MiB；GB 级传输需要流式实现。文本必须为 UTF-8。文件保存使用同目录临时文件和原子重命名，服务端需支持此能力。保留原文件权限位，但不保证所有者、ACL、扩展属性或硬链接关系不变。
- 文件根目录对 SFTP 生效，解析路径和符号链接。它不是操作系统级沙箱，无法阻止远端其他进程制造文件系统竞态。
- 在服务器编辑窗口的「MCP 访问权限」中，可关闭整台服务器的 MCP 访问，或分别授权 17 项操作：浏览目录、读取文本、预览、下载、文本写入、精确编辑、上传、创建目录、重命名 / 移动、删除、命令执行，以及终端会话列表、创建、读取输出、输入、调整尺寸和关闭。关闭 MCP 访问后不再向 MCP 列出该服务器，已知 ID 也不能调用。桌面操作不受这些 MCP 授权限制。
- 新增服务器默认仅授予四项文件读取权限。旧配置自动保留原三个开关对应的权限，编辑保存后使用细分授权；权限保存在 SQLite，保存后每次 MCP 操作重新校验，无需重启服务。`list_servers` 返回的 `mcp_permissions` 为当前允许的工具名；旧 `allow_*` 字段仅作兼容摘要。
- **exec 和终端拥有 SSH 账户的完整 shell 权限，不受 SFTP 根目录限制。** 细分授权按工具入口生效，例如禁用下载不会阻止已授权的文本读取或命令读取文件；全局删除保护、危险命令拦截、只读与审批策略继续生效。
- 「设置 → 命令与 Shell 权限」可独立控制诊断白名单、管道、重定向、组合命令、脚本、命令替换和 MCP 交互终端，不需要为了使用管道而关闭高危拦截。Shell 命令按语法树检查，已知高危命令在管道、组合命令及可解析的嵌套 Shell 中仍会被拒绝。外部脚本、任意解释器代码和交互终端输入无法完整静态检查，具体规则见 [安全控制](docs/SECURITY_CONTROLS.md)。
- 停用服务器会阻止后续操作，已有终端不会自动终止，仍可在桌面关闭会话。停止 MCP 会阻止 MCP 连接，桌面终端可继续使用。
- 「持久终端」指应用运行期间持续连接的 SSH PTY，能够保留 cd、环境变量与交互进程。当前不支持断网重连或跨应用重启恢复，如需此能力可进一步集成远端 tmux。
- 默认最多保留 32 个终端，可在设置中调整为 1–64；降低上限不会关闭已有会话。每个会话保留最近 1 MiB 输出，更早输出可能被覆盖。
- 设置支持连接超时（1–120 秒）、SSH 读写超时（1–300 秒）和默认命令超时（1–300 秒），保存后用于后续操作。命令可通过 timeout_secs 单次覆盖默认时限。外观、启动页等界面偏好保存在本机浏览器存储；浏览器演示的业务设置仅保存在页面内存中。
- 当前不包含跳板机、递归目录传输、逐命令人工审批、命令隔离、SSH config 导入或 tmux 恢复。

## 验证

```powershell
npm.cmd run build
npm.cmd test
# 先构建无窗口后端，再进行真实协议测试：
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/cargo.ps1 build --manifest-path src-tauri/Cargo.toml --no-default-features --bin remote-desk-headless
npm.cmd run test:e2e
```

核心测试覆盖配置持久化、主机变更清除信任、损坏配置保护、MCP 权限拒绝、敏感内容不进入日志以及 MCP 启停 / 端口复用。

协议测试在 127.0.0.1 创建一次性 SSH/SFTP 服务，使用真正的 MCP JS 客户端连接 Rust 服务，验证认证、工具发现、SSH exec、SFTP 增删读写、二进制传输、符号链接越界、持久 PTY、超时与审计，不连接你的服务器。它的远端命令 / 文件语义由内存测试服务实现，不能代替真实 Linux / OpenSSH 服务器验收。报告保存在 .artifacts/integration-*/report.json。

## 项目结构

```text
src/
  App.tsx                  工作空间、服务器、MCP、审计与设置
  components/              服务器表单、远程文件、命令与终端
  api.ts                   Tauri IPC 和独立的浏览器演示
src-tauri/src/
  database.rs              SQLite 表结构、事务与旧版自动迁移
  model.rs                 配置与操作数据结构
  state.rs                 共享状态、配置、权限、审计
  ssh.rs                   SSH 指纹 / 认证、exec 与 SFTP
  terminal.rs              PTY 会话、输出缓冲与输入队列
  mcp.rs                   18 个 MCP 工具、认证和服务生命周期
  mcp_args.rs              各工具独立输入 Schema，映射到类型化文件请求
  file_tools.rs            有界分页读取、目录分页、批量文本编辑和 diff
  lib.rs                   桌面命令与应用入口
  bin/headless.rs          无窗口 MCP 调试入口
src-tauri/tests/core.rs     后端核心测试
scripts/integration.mjs     MCP → Rust → SSH 协议测试
```

实现调研与取舍见 docs/RESEARCH.md。
