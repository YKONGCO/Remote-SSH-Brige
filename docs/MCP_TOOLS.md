# MCP 工具参考

协议名称和客户端配置键为 `remote-ssh`。当前提供 16 个工具。先调用 `list_servers` 获取精简的服务器识别信息；需要权限、传输配置或命令规则时调用 `server_info`，分别指定 `topic=permissions/transfers/commands`。完整白名单不在发现阶段返回。详细恢复行为见 [传输与恢复](TRANSFER_RECOVERY.md)。

## 参数

### SSH 端口转发

`port_forward` 支持以下动作，各动作分别需要 `port_forward_open`、`port_forward_list`、`port_forward_read`、`port_forward_close` 权限。已有配置不会自动获得新权限。

| action  | 参数                                                | 说明                                                              |
| ------- | --------------------------------------------------- | ----------------------------------------------------------------- |
| `open`  | `server_id, remote_port, local_port?, remote_host?` | 创建本地 TCP 转发；本地端口默认 0，由系统分配                     |
| `list`  | `server_id`                                         | 仅列出该服务器 `running/closing` 的转发；已关闭记录仍可按 ID 读取 |
| `read`  | `server_id, tunnel_id`                              | 获取状态、连接数、双向流量和最近错误                              |
| `close` | `server_id, tunnel_id`                              | 关闭监听及已有连接，可重复调用                                    |

```json
{
  "action": "open",
  "server_id": "your-server-id",
  "remote_port": 8080,
  "local_port": 18080
}
```

映射为应用所在电脑的 `127.0.0.1:18080` → SSH 服务器的 `127.0.0.1:8080`。MCP 客户端位于另一台电脑时，返回的本地地址仍属于应用所在电脑。`remote_host` 仅允许 `127.0.0.1`（默认）或 `::1`；仅支持 TCP 本地转发，不支持 UDP、反向转发或 SOCKS。

`running` 表示本地监听已创建且 SSH 身份验证通过；每个入站连接再建立独立 SSH 会话和 `direct-tcpip` 通道，远端服务不可用或禁止转发时，会关闭该连接并记录最近错误。最多同时运行 32 个转发，每个转发最多 16 个连接。`bytes_sent`/`bytes_received` 为成功写出的流量计数。`close` 可能先返回 `closing`，待正在进行的 SSH 握手结束后变为 `closed`。

创建受全局只读和敏感操作审批控制。修改服务器配置、撤销权限、开启只读、删除服务器时关闭相关转发；停止 MCP 关闭 MCP 创建的转发，退出应用关闭全部转发。状态保存在内存中，重启后不会恢复。审计保存映射和操作结果，不保存流量正文。

### 通用参数

`?` 表示可省略，其余字段必填。文件工具使用远端 POSIX 绝对路径，禁止 NUL、反斜杠和 `..`，解析后须位于服务器的文件根目录内。`exec` 和终端不受文件根目录限制。

| 工具             | 参数                                                                               | 默认值与行为                                              |
| ---------------- | ---------------------------------------------------------------------------------- | --------------------------------------------------------- |
| `list_servers`   | `{}`                                                                               | 仅返回已启用且开放 MCP 的服务器，不包含凭据               |
| `server_info`    | `server_id, topic?`                                                                | 按需查看权限、传输设置或命令规则                          |
| `transfer_batch` | `server_id, direction, items, recursive?, overwrite?, skip_existing?, request_id?` | 批量或递归传输，返回分组与子任务                          |
| `exec`           | `server_id, command, timeout_secs?`                                                | 省略或 `0` 使用应用默认超时，否则为 1–300 秒              |
| `job`            | `server_id, action` 及对应参数                                                     | 列出、增量读取或关闭超时后保留的执行会话                  |
| `port_forward`   | `server_id, action` 及对应参数                                                     | 创建、列出、查看或关闭本地 TCP 转发                       |
| `read_file`      | `server_id, path, offset?, limit?`                                                 | `offset=1`，`limit=2000`，最多 50 KiB 正文                |
| `list_files`     | `server_id, path, offset?, limit?`                                                 | 目录分页，`offset=0`、`limit=200`，最多 1000 项及 50 KiB  |
| `stat_file`      | `server_id, path`                                                                  | 查询文件或目录属性，不读取正文                            |
| `write_file`     | `server_id, path, content, mode?, create_parents?`                                 | `mode=overwrite`；`create` 拒绝已有目标；默认不创建父目录 |
| `edit_file`      | `server_id, path, edits`                                                           | 1–100 组 `old_text/new_text`，全部成功后提交              |
| `remove_path`    | `server_id, path, recursive?, missing_ok?`                                         | `recursive=false`，`missing_ok=true`                      |
| `terminal`       | `action` 及对应参数                                                                | 见终端动作表                                              |
| `upload_file`    | `server_id, local_path, remote_path, overwrite?, request_id?`                      | 启动可恢复上传，默认不覆盖                                |
| `download_file`  | `server_id, local_path, remote_path, overwrite?, request_id?`                      | 启动可恢复下载，默认不覆盖                                |
| `transfer`       | `server_id, action` 及对应参数                                                     | 读取传输列表、进度和结果，或请求取消                      |

标识、路径、命令和 `old_text` 不得为空；`content` 和 `new_text` 可为空。`list_files` 和 `stat_file` 通过 SFTP 查询，不要求 `exec`，各需同名权限，界面归入“文件读取”。已有显式授权列表不会自动增加这两项权限，可在服务器设置中重新勾选“文件读取”。

`list_files` 返回 `entries`、`has_more`、`next_offset` 和截断原因。目录排在文件前，其余按名称排序；最多扫描 10000 项。每项包含名称、路径、目录/链接标志、字节大小、Unix 修改时间及八进制权限。`stat_file` 另返回类型、访问时间、uid/gid；符号链接的目标也必须位于授权根目录内，属性来自解析后的目标。

`write_file.create_parents=true` 会在授权根目录内逐级创建缺失父目录（0755，受远端 umask 约束），不需要 `exec`。默认 false，父目录缺失返回 `PARENT_NOT_FOUND`。路径和已有符号链接须通过根目录检查；后续写入失败时，已创建的目录可能保留。

## 文件流传输

新增 `upload_file`、`download_file`、`transfer`。启动工具参数一致：

```json
{
  "server_id": "服务器 ID",
  "local_path": "D:/files/archive.zip",
  "remote_path": "/srv/files/archive.zip",
  "overwrite": false
}
```

两个路径都是完整文件路径。`local_path` 属于运行本应用的电脑，不是 MCP 客户端的电脑；`remote_path` 受服务器文件根目录约束。`overwrite` 默认 false，拒绝已有目标。父目录必须已经存在，不自动创建目录。工具拒绝 `content`、`base64` 和分块数据字段。

`upload_file` 要求 `upload_file` 和 `transfer` 两项服务器权限，以及全局 `local_upload_roots` 授权目录；`download_file` 要求 `download_file`、`transfer` 和 `local_download_roots`。两类目录默认空，即禁止 MCP 本地文件访问。路径按真实路径校验符号链接；下载不覆盖符号链接。桌面手动选择的文件不受 MCP 本地目录列表限制。`list_servers.file_transfer` 返回限额及该服务器已授权方向的本地目录。

启动通过本地参数校验后返回 `status=queued`、`transfer_id` 和初始元数据，随后由本地 worker 直接在磁盘与 SFTP 之间传输，不携带文件正文。远端路径、SSH 连接或提交失败可能在后台返回；只有最终 `completed` 才表示文件提交成功。

| `transfer.action`  | 参数                                                | 行为                                                                                                |
| ------------------ | --------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `list`             | `server_id`                                         | 返回 `transfers` 元数据列表                                                                         |
| `read`             | `server_id, transfer_id, version?, wait_secs?`      | `wait_secs` 默认 0，范围 0–30；不带版本时等待结束或超时，带当前版本时等待下一次变化，旧版本立即返回 |
| `cancel`           | `server_id, transfer_id`                            | `cancel_requested=true` 表示接受取消；继续读取最终结果，提交阶段拒绝取消                            |
| `pause`            | `server_id, transfer_id`                            | 请求暂停，等待最终 `paused`                                                                         |
| `resume` / `retry` | `server_id, transfer_id`                            | 重新校验并恢复暂停或失败任务                                                                        |
| `configure`        | `server_id, transfer_id, priority?, bandwidth_kib?` | 调整优先级和每任务限速                                                                              |
| `cleanup`          | `server_id, transfer_id`                            | 停止后显式清理临时文件和记录；远端清理需要删除授权                                                  |

```json
{
  "server_id": "服务器 ID",
  "action": "read",
  "transfer_id": "transfer_...",
  "version": 5,
  "wait_secs": 10
}
```

快照包括 `status`、`phase`、字节计数、速度、耗时、版本、路径和错误摘要，另有 `retry_count/next_retry_at/resumed_bytes/priority/bandwidth_kib/sha256/group_id`。完成后保留最后的流传输平均速度；空文件为 0。状态为 `queued/running/retrying/paused/completed/failed/cancelled`，阶段还包括等待审批、连接、校验和提交。100% 字节进度仍可能正在提交，不能当成完成。可靠结果以返回快照为准。

每次管理操作检查服务器权限，ID 不能跨服务器访问。默认全局运行 4 个任务，每台主机运行 2 个，其余排队；同一目标只允许一个活动任务。SQLite 持久化最多 1024 个任务，重启后中断任务恢复为暂停。成功记录保留 7 天，失败及暂停任务保留至显式清理。大小上限支持 MiB/GiB 和 0（不限大小）；文本和旧式内联传输另有 256 MiB 保护。

传输使用固定文件缓冲并持久化分块摘要，失败或暂停保留临时文件用于续传。网络故障自动退避重试；源文件变化、权限或空间错误停止。恢复时校验保留块，回退未确认尾部。文件完整且通过 SHA-256 校验后才提交正式路径。上传使用 OpenSSH SFTP 原子扩展。提交结果未知时，手动重试先核验目标摘要，避免重复覆盖。审批继续生效，取消或暂停可能等待当前 I/O 超时。

数据库仅保存传输参数、开始和结束时间、字节数、状态及错误摘要，不存正文、Base64 或分块。最终审计在文件提交后写入；若审计写入失败，保留实际文件结果并返回 `audit_error`。

## 命令执行

`exec` 返回 `stdout`、`stderr`、`exit_code` 和 `truncated`。权限在运行时校验，命令规则可通过 `server_info(topic=commands)` 按需查看。管道、重定向、组合命令、脚本和命令替换分别授权。高危拦截开启时，即使允许 Shell 功能，也会拒绝已知高危命令。

设置中的「命令超时后保留为 Job」（`exec_timeout_to_job`）默认关闭，仅影响之后启动的命令。开启且服务器独立授予 `job` 权限后，等待超时将保留同一个 SSH Session / Channel，成功返回 `job_id`、`status`、已收到的输出及 `next_stdout_cursor` / `next_stderr_cursor`。成功表示会话已保留，命令是否成功仍以最终 `exit_code` 为准。命令只执行一次，无需远端后台进程、文件或重新连接。

未开启、未授权或已达到每台服务器 4 个活动 Job 上限时，超时关闭 SSH 执行连接并返回错误；`details` 包含已收到的输出、游标和 `reason`。关闭连接不保证远端派生进程终止。

## Job 动作

所有动作都要求 `server_id` 和独立 `job` 权限，每次调用重新检查服务器授权。

| `action` | 其他参数                                             | 行为                                                             |
| -------- | ---------------------------------------------------- | ---------------------------------------------------------------- |
| `list`   | 无                                                   | 返回 `jobs` 数组，包含 ID、服务器、创建时间、状态和退出码        |
| `read`   | `job_id, stdout_cursor?, stderr_cursor?, wait_secs?` | 游标默认 0；最多等待 0–30 秒（默认 0），收到输出或命令结束就返回 |
| `close`  | `job_id`                                             | 关闭这次 SSH 执行连接，不发送 `kill`，已结束会话保留原结果       |

```json
{
  "server_id": "服务器 ID",
  "action": "read",
  "job_id": "job_...",
  "stdout_cursor": 1024,
  "stderr_cursor": 0,
  "wait_secs": 10
}
```

`read` 返回 stdout / stderr、下次字节游标、当前状态和退出码。游标为原始字节偏移，不能用字符串字符数计算；使用上次返回的游标续读。输出按 UTF-8 解码，无效字节使用替换字符。每路保留最近 4 MiB，旧游标被覆盖时返回 `stdout_truncated` / `stderr_truncated` 和当前 `*_start_cursor`；超过已收到输出的游标报错。`*_end_cursor` 表示已收到的总字节数；`*_has_more` 表示仍有未返回字节（例如等待完整 UTF-8 字符）。

状态为 `running`、`exited`、`closed`、`disconnected`；只有 `exited` 提供命令退出码，非零表示命令失败，断线不伪造退出码。正常结束、主动关闭和断线的会话保留 30 分钟。连接和输出仅放在本地内存，worker 持续排空两路输出；退出应用关闭连接，重启无法恢复会话。SQLite 只保存有限审计，移交与最终结果通过 `job_id` 关联。

客户端提供 `_meta.progressToken` 时，前台 `exec` 通过 MCP `notifications/progress` 发送输出块（每路每块最多 8 KiB，最多每 100 ms 一次，队列有界）。通知是否显示由客户端决定，可靠输出保留在本地缓冲区。`exec` 返回后停止原请求的通知，后续使用 `job.read`；读取等待期间也可发送 progress。

## 文本读取

`read_file` 从第 `offset` 行开始读取，行号从 1 开始，`limit` 范围为 1–2000。正文保留原始缩进和换行，不添加行号前缀；行数、50 KiB 正文或扫描预算先达到者生效。

MCP `content` 仅提供原始文本；`structuredContent` 提供 `path`、`start_line`、`end_line`、`returned_lines`、`has_more`、`next_offset`、`total_lines`、`truncated`、`truncated_by`、`first_line_exceeds_limit` 和 `scanned_bytes` 等元数据，不重复正文。

存在后续内容时，使用 `next_offset` 续读。`total_lines` 仅在扫描到 EOF 后有值；空文件的行号为 null，超出 EOF 返回 `OFFSET_OUT_OF_RANGE`。起始行超过输出预算时不返回部分行，`next_offset=null`，需要有界 `exec` 检查。

扫描受下载上限约束，无法定位起始行时返回 `READ_SCAN_LIMIT`。非 UTF-8 文本或含 NUL 的内容返回 `UNSUPPORTED_TEXT`。图片需下载后使用本地应用查看；桌面编辑器另行读取完整原文。

## 写入与编辑

`write_file` 的 `content` 必填，空字符串可写空文件。`overwrite` 创建或完整覆盖文件；`create` 拒绝已有目标，包括符号链接。父目录必须存在，否则返回 `PARENT_NOT_FOUND`；大小受上传上限约束。

`edit_file` 需要同时授予 `read_file` 和 `edit_file` 权限。调用示例：

```json
{
  "server_id": "服务器 ID",
  "path": "/srv/app/config.json",
  "edits": [{ "old_text": "\"port\": 8080", "new_text": "\"port\": 9090" }]
}
```

每项匹配同一份原文，`old_text` 须唯一，各区间不得重叠。全部匹配成功后提交一次；任一项失败均不落盘，`new_text` 为空表示删除片段。

匹配先去 BOM、归一化换行并精确查找；失败后进行有限的 Unicode 与空白归一化。写回恢复 BOM 和检测到的换行样式，混合换行可能统一；模糊匹配只回写变化行块。无变化返回 `EDIT_NO_CHANGE`，其他匹配错误包括 `EDIT_NOT_FOUND`、`EDIT_AMBIGUOUS` 和 `EDIT_OVERLAP`。

原文受下载上限约束，请求与最终文件受上传上限约束。返回路径、应用数量、首个变化行、写入字节数、模糊匹配标志和最多 50 KiB 的 diff；截断通过 `diff_truncated` 标识。

## 提交与并发

文本写入、编辑和桌面上传使用同目录临时文件。覆盖依赖 `posix-rename@openssh.com`，仅创建依赖 `hardlink@openssh.com`；不支持时分别返回 `ATOMIC_WRITE_UNSUPPORTED` 或 `ATOMIC_CREATE_UNSUPPORTED`，不会降级为先清空目标。

保留原权限位，但不保证所有者、ACL、扩展属性、硬链接关系或 fsync 持久性不变。提交期间断连可能返回 `COMMIT_STATUS_UNKNOWN`，应先读回核验再决定是否重试。

同一服务器配置的应用内 SFTP 修改共用队列，审批等待不持锁，获批获锁后复查权限和路径。队列不协调 `exec`、终端、外部进程或其他服务器配置，不提供跨进程事务。业务错误返回 `isError=true` 及错误信息，不回显文件片段。

## 安全删除

`remove_path` 必须单独授权，并关闭全局删除保护；不需要授予 `exec`。示例：

```json
{
  "server_id": "服务器 ID",
  "path": "/srv/app/cache",
  "recursive": true,
  "missing_ok": true
}
```

默认只删除指定文件、链接或空目录；非空目录需显式递归。不展开通配符，不跟随最终链接或递归中的链接。禁止删除授权根目录自身、`/` 和 `/tmp`、`/etc`、`/var` 等系统顶层目录。

递归先完整预检，最多 10000 个条目、深度 64；预检失败不删除。执行前复查父目录、类型与属性，但无法完全消除外部进程竞态，也不支持事务回滚。

成功返回 `path`、`removed`、`missing`、`removed_entries`、`kind` 和 `recursive`。默认目标缺失视为成功；`missing_ok=false` 时返回 `NOT_FOUND`。执行中失败的 `details` 包含 `failed_path`、`removed_entries`、`partial` 和 `outcome_unknown`；响应丢失可能返回 `COMMIT_STATUS_UNKNOWN`，应先检查实际状态。

## 终端动作

| `action` | 其他参数                            | 默认值 / 授权键                                |
| -------- | ----------------------------------- | ---------------------------------------------- |
| `list`   | 无                                  | `list_terminals`，仅列出有权限的服务器会话     |
| `open`   | `server_id, cols?, rows?`           | 100 × 30；`terminal_open`                      |
| `read`   | `server_id, session_id, cursor?`    | `cursor=0`；`terminal_read`                    |
| `write`  | `server_id, session_id, content`    | 原始输入，执行命令需包含换行；`terminal_write` |
| `resize` | `server_id, session_id, cols, rows` | 列 20–500、行 5–200；`terminal_resize`         |
| `close`  | `server_id, session_id`             | `terminal_close`                               |

`read` 的工具正文直接返回 UTF-8 文本，不返回 Base64。`structuredContent` 包含 `cursor`、`alive`、`truncated` 和 `has_more`，不重复包含正文。每次最多读取 64 KiB 原始字节，游标使用原始字节偏移。继续读取时传入返回的 `cursor`；即使 `alive=false`，也需读到 `has_more=false` 才表示剩余输出已读完。缓冲区覆盖旧输出时 `truncated=true`。未完整的 UTF-8 尾部字符保留到后续读取，无效字节使用替换字符。

`list` 只传 `action`，不接受 `server_id`。MCP 打开终端和发送输入还需开启全局「允许 MCP 交互终端」。

## 权限与审计

21 项授权分别控制目录列表、属性查询、读取、写入、编辑、安全删除、命令、Job、三项文件传输权限、四种端口转发动作和六种终端动作。新服务器默认仅允许目录列表、属性查询和读取，旧命令授权不自动授予 Job 或文件传输权限；保存服务器时拒绝未知或退役权限名。全局只读与审批可以进一步限制已授予操作。

## 返回格式

普通数据及运行时业务错误在 `content` 的单个文本块中返回 JSON，不复制到 `structuredContent`。`read_file` 和 `terminal.read` 的文本块只包含原始正文，分页、截断和游标信息放在 `structuredContent`，不向正文追加提示。读取文件时检查 `has_more` 和 `next_offset`；`first_line_exceeds_limit` 或 `truncated_by=scan_bytes` 时，可在已授权的情况下改用有界 `exec` 检查。

`isError` 表示工具调用是否失败。命令返回非零 `exit_code`、状态查询返回 `failed/cancelled/disconnected`，不等同于查询调用失败。传输进度以返回快照为准，不另发送相同的完整快照通知。

### 错误返回

`exec` 被命令策略拒绝时，返回精简 JSON，例如：

```json
{
  "code": "COMMAND_POLICY_DENIED",
  "message": "Compound commands are not allowed",
  "retryable": false,
  "hint": "Review this executable or shell rule in server command settings"
}
```

包含错误码、简短英文原因、可重试标记和必要提示。不回显完整长命令、策略配置、保护开关或错误链。审计中保留命令和诊断，策略拒绝不保存整份策略配置。

其他运行时业务错误同样使用 `isError=true` 及精简 JSON。`message` 为单行，最多 240 个字符。必要时包含缺少的权限名；删除部分完成或结果未知时保留核验提示和部分结果字段。Schema 错误仍使用简短参数说明，删除冗长候选字段列表，例如 ``Invalid arguments: missing field `path` ``。

HTTP 拒绝保持原状态码，并返回简短英文正文，例如 401 的 `Missing or invalid MCP bearer token`、403 的 `Browser-origin MCP requests are not allowed`、413 的 `Request body exceeds the configured size limit`。远端程序的 stdout/stderr、文件正文和用户提供的路径保持原样。

后台传输和会话查询保留正常 JSON 状态字段；对象形式的 `error`、`last_error` 压缩为相同的错误 JSON，原本为字符串的错误保持简短文字。完整诊断留在审计记录，包含 `code`、`stage`、`message`、`details`。

审计中的权限拒绝记录保留缺失授权及服务器开关，执行错误保留 SSH/SFTP 或系统错误码及相关操作标识。MCP 错误提示只说明缺少的授权或具体执行原因。

审批超过 `server_info(topic=permissions)` 返回的 `approval_timeout_secs`（1–120 秒，默认 120）自动拒绝。用户主动拒绝时说明审批已拒绝。完整策略按需查询，不在每次错误中重复发送。

编辑审批展示替换请求前 8192 个字符；删除审批展示递归与缺失处理选项。完整请求绑定到单次审批。审计不保存直接文件正文、编辑原文、新文本或 diff，但会保存命令及其有界输出。完整策略见 [安全控制](SECURITY_CONTROLS.md)。
