# MCP 工具设计：精简的 SSH Agent 接口

状态：已实现，当前版本 0.3.1，工具接口沿用 0.3.0。日期：2026-10-31。

产品名为 Remote SSH MCP，协议名称和客户端配置键为 remote-ssh；旧 remote-desk 配置通过一键配置备份迁移，数据、凭据和应用内部标识保持。

## 设计依据

参考 [earendil-works/pi 源码](https://github.com/earendil-works/pi/tree/2a46c4208004c4c5a41b75d775710f774ad9b630/packages/coding-agent/src/core/tools)，沿用读取、完整写入、局部编辑、命令执行分工，以及独立 Schema、分页截断、原文匹配和修改队列。实现为 Rust / SSH 适配，不以教程为依据。

read.ts 使用从 1 开始的行号分页，正文不逐行添加行号。edit.ts / edit-diff.ts 支持原文批量匹配、有限模糊回退、BOM 和换行处理。本项目增加 SFTP 根目录、逐项权限、审批、审计脱敏和原子提交。

0.3.0 将 18 个工具精简为 7 个。目录、搜索、移动、创建目录、删除及二进制传输统一使用 exec，按现有策略调用 ls/find/grep/mv/mkdir/rm/base64 等远端命令。保留 preview_file 供文本/图片预览；终端合并为一个 terminal 工具，保留持久会话状态。

桌面文件管理继续使用原有 SFTP 浏览、建目录、重命名、删除、上传和下载。这些内部操作不再注册为 MCP 工具，也不能通过已知旧工具名绕过路由。

## 对外参数

`?` 表示可省略，其余必填。每个工具及 terminal 各动作均拒绝无关字段。标识、路径、命令和 old_text 不得为空；content、new_text 可为空。

| 工具         | 参数                              | 默认值 / 行为                                                 |
| ------------ | --------------------------------- | ------------------------------------------------------------- |
| list_servers | {}                                | 发现服务器、操作授权、terminal_actions 和安全策略，不返回凭据 |
| exec         | server_id, command, timeout_secs? | 超时默认 0 使用应用配置，范围 0–300 秒                        |
| read_file    | server_id, path, offset?, limit?  | offset=1，limit=2000，最多 50 KiB 正文                        |
| write_file   | server_id, path, content, mode?   | mode=overwrite，可选 create；父目录须存在                     |
| edit_file    | server_id, path, edits            | 1–100 组 old_text/new_text，匹配同一原文，全部成功后提交      |
| preview_file | server_id, path                   | 文本/图片预览，最多 2 MiB 且受下载限制                        |
| terminal     | action 及对应参数                 | 见下表                                                        |

文件工具使用远端 POSIX 绝对路径，禁止 NUL、反斜杠和 ..；realpath 后必须在配置 root 内。exec 与 terminal 使用 SSH 账号的 Shell 权限，不受 SFTP 根目录限制。

| terminal action | 其他参数                          | 默认值 / 授权键                            |
| --------------- | --------------------------------- | ------------------------------------------ |
| list            | 无                                | list_terminals，只返回允许列出的服务器会话 |
| open            | server_id, cols?, rows?           | 100 × 30；terminal_open                    |
| read            | server_id, session_id, cursor?    | cursor=0；terminal_read                    |
| write           | server_id, session_id, content    | 输入含换行才执行命令；terminal_write       |
| resize          | server_id, session_id, cols, rows | 列 20–500、行 5–200；terminal_resize       |
| close           | server_id, session_id             | terminal_close                             |

terminal 使用按 action 区分的 oneOf Schema，不把所有字段都设为可选；write 必须有 content，list 不接受 server_id。终端输出沿用 Base64 字节、下一 cursor、alive 和 truncated，与桌面共享会话。

```json
{
  "action": "write",
  "server_id": "prod",
  "session_id": "返回的会话 ID",
  "content": "cd /srv/app\npwd\n"
}
```

## 分段读取

read_file 流式扫描，offset 从 1 开始，limit 为 1–2000。正文保留完整行、原始缩进和换行，不加行号前缀。行数、正文 50 KiB 或扫描预算先达到者生效，另预留最多 1 KiB 续读提示。

MCP content 提供正文和提示；structuredContent 仅提供 path、start_line、end_line、returned_lines、has_more、next_offset、total_lines、truncated、truncated_by、first_line_exceeds_limit、scanned_bytes，不重复正文。

有剩余内容时传 next_offset 续读；total_lines 仅在实际扫描到 EOF 后有值。空文件返回空正文及 null 行号，末尾单个换行不制造额外行。offset 超过 EOF 返回 OFFSET_OUT_OF_RANGE。

起始行超过输出预算时不返回部分行，next_offset=null，提示有界 exec 检查（需相应授权），避免无限续读同一行。扫描受 download_limit 约束；无法定位 offset 时返回 READ_SCAN_LIMIT。达到扫描预算后，后续分页可能仍需 exec 检查。

读取片段非 UTF-8 或含 NUL 返回 UNSUPPORTED_TEXT；图片使用 preview_file。桌面编辑器保留完整原文读取，不套用 MCP 输出预算。

## 写入与批量编辑

write_file 的 content 必填且允许为空；overwrite 创建或完整覆盖，create 拒绝已有目标（包括符号链接）。父目录缺失返回 PARENT_NOT_FOUND；MCP 使用已授权 exec 创建，桌面保留原建目录入口。大小受 upload_limit 约束。

```json
{
  "server_id": "prod",
  "path": "/srv/app/config.json",
  "edits": [
    { "old_text": "\"port\": 8080", "new_text": "\"port\": 9090" },
    { "old_text": "\"debug\": true", "new_text": "\"debug\": false" }
  ]
}
```

每项匹配同一原文，old_text 必须非空、唯一，各区间不得重叠；验证全部成功后倒序替换并提交一次。任一项失败都不落盘，new_text 为空表示删除片段。

先去 BOM 并归一化换行为 LF，再优先精确匹配；失败后回退到有限 NFKC、行尾空白、智能引号、横线及特殊空格归一化。唯一性在归一化空间检查；任一项需要模糊匹配时全部项在相同空间重新定位，不使用任意编辑距离搜索。

模糊匹配只回写变化行块，保留未修改行内容。写回恢复 BOM 与检测到的换行样式，混合换行可能统一。无变化返回 EDIT_NO_CHANGE；其他错误包括 EDIT_NOT_FOUND、EDIT_AMBIGUOUS、EDIT_OVERLAP。原文受 download_limit 约束，请求及最终文件受 upload_limit 约束。

返回 path、applied_edits、first_changed_line、written_bytes、used_fuzzy_match 及最多 50 KiB diff，截断标记 diff_truncated。

## 提交与并发

写入、编辑及桌面上传共用同目录临时文件提交。覆盖使用 posix-rename@openssh.com，不支持则 ATOMIC_WRITE_UNSUPPORTED。仅创建使用 hardlink@openssh.com 链接完整临时文件后清理临时名称，不支持则 ATOMIC_CREATE_UNSUPPORTED；并发创建者不会被覆盖。通过独立 SFTP 子系统发送扩展，不降级为先清空目标再写入或 Shell 命令。

保留原权限位，不承诺所有者、ACL、扩展属性、硬链接关系或 fsync 持久性不变。返回兼容桌面的 written，以及 written_bytes、created；overwrite 的 created 基于提交前检查，无法识别外部进程在检查与提交间的创建/删除。

同一 server_id 的应用内 SFTP 修改共用队列，包含桌面目录移动/删除。审批等待不持锁，获批获锁后复查权限和路径；锁覆盖读取、验证和提交。即使异步调用者已断连，也要等已发出的阻塞 SFTP 请求结束再释放锁。该队列不协调 exec、terminal、外部进程或其他服务器配置，不提供跨进程 compare-and-swap。

提交期间断连或响应不完整返回 COMMIT_STATUS_UNKNOWN，先读回核验，不自动重试。业务错误返回 isError=true 及 code/message；缺失字段、未知字段和无效 action 由框架拒绝。错误不回显文件片段。

## 权限、审批与升级

权限界面保留 11 个授权项：read_file、preview_file、write_file、edit_file、exec 与六个终端动作。edit_file 同时要求 read_file；默认新服务器仅授权读取和预览。终端授权沿用 list_terminals、terminal_open 等内部键，合并工具不隐含开放所有动作。

list_servers.mcp_permissions 返回操作授权，terminal_actions 明确返回 terminal 可用动作。退役工具授权在加载后不对外生效，保存时清理；不自动授予 exec，不改变命令策略。mkdir/mv 仍可能被诊断白名单拒绝，rm 在高危拦截开启时仍被拒绝。只读、命令保护和审批继续生效；桌面删除保护不等于 Shell 删除保护。

编辑审批展示全部替换请求前 8192 字符；完整请求和服务器配置绑定单次审批。审计仅保存路径、数量、字节、结果与耗时，不保存原文、新文本或 diff。命令及其输出沿用原 exec 审计规则。

升级 0.3.0 后客户端需重新连接并发现工具。六个旧终端工具名改为 terminal/action，旧目录和二进制传输工具改为 exec。0.2.0 的读取、写入、编辑和预览参数保持。

## 验证范围

自动化覆盖严格 Schema、各 terminal action 的字段及授权、旧工具不可调用、退役授权清理且不扩权、分页预算、UTF-8/CRLF/BOM、批量失败不落盘、原子创建竞争、扩展不支持、提交响应丢失、并发编辑和审计脱敏。

MCP JS 客户端连接真实 Rust HTTP 服务及临时加密 SSH/SFTP fixture。fixture 文件系统和命令由内存服务模拟；exec 的 ls/mv 等检查只验证命令进入 SSH，不模拟完整 Unix 工具语义。真实目标 OpenSSH 扩展兼容性仍需实际环境验收。
