# MCP 工具参考

协议名称和客户端配置键为 `remote-ssh`。当前提供 8 个工具；每个工具及终端动作都有独立 Schema，拒绝无关字段。先调用 `list_servers` 获取服务器 ID、`mcp_permissions`、`terminal_actions` 和 `security` 策略。

## 参数

`?` 表示可省略，其余字段必填。文件工具使用远端 POSIX 绝对路径，禁止 NUL、反斜杠和 `..`，解析后须位于服务器的文件根目录内。`exec` 和终端不受文件根目录限制。

| 工具           | 参数                                       | 默认值与行为                                 |
| -------------- | ------------------------------------------ | -------------------------------------------- |
| `list_servers` | `{}`                                       | 仅返回已启用且开放 MCP 的服务器，不包含凭据  |
| `exec`         | `server_id, command, timeout_secs?`        | 省略或 `0` 使用应用默认超时，否则为 1–300 秒 |
| `read_file`    | `server_id, path, offset?, limit?`         | `offset=1`，`limit=2000`，最多 50 KiB 正文   |
| `write_file`   | `server_id, path, content, mode?`          | `mode=overwrite`；`create` 拒绝已有目标      |
| `edit_file`    | `server_id, path, edits`                   | 1–100 组 `old_text/new_text`，全部成功后提交 |
| `remove_path`  | `server_id, path, recursive?, missing_ok?` | `recursive=false`，`missing_ok=true`         |
| `preview_file` | `server_id, path`                          | 文本或图片预览，最多 2 MiB，受下载上限约束   |
| `terminal`     | `action` 及对应参数                        | 见终端动作表                                 |

标识、路径、命令和 `old_text` 不得为空；`content` 和 `new_text` 可为空。目录列表、搜索、移动、创建目录及二进制传输可通过已授权的 `exec` 调用远端命令，仍受命令策略、只读和审批约束。桌面文件管理提供对应的 SFTP 操作。

## 命令执行

`exec` 返回 `stdout`、`stderr`、`exit_code` 和 `truncated`。调用前检查 `list_servers.security.command_policy`；管道、重定向、组合命令、脚本和命令替换分别授权。高危拦截开启时，即使允许 Shell 功能，也会拒绝已知高危命令。

## 文本读取

`read_file` 从第 `offset` 行开始读取，行号从 1 开始，`limit` 范围为 1–2000。正文保留原始缩进和换行，不添加行号前缀；行数、50 KiB 正文或扫描预算先达到者生效。

MCP `content` 提供原始文本及可选续读提示；`structuredContent` 提供 `path`、`start_line`、`end_line`、`returned_lines`、`has_more`、`next_offset`、`total_lines`、`truncated`、`truncated_by`、`first_line_exceeds_limit` 和 `scanned_bytes` 等元数据，不重复正文。

存在后续内容时，使用 `next_offset` 续读。`total_lines` 仅在扫描到 EOF 后有值；空文件的行号为 null，超出 EOF 返回 `OFFSET_OUT_OF_RANGE`。起始行超过输出预算时不返回部分行，`next_offset=null`，需要有界 `exec` 检查。

扫描受下载上限约束，无法定位起始行时返回 `READ_SCAN_LIMIT`。非 UTF-8 文本或含 NUL 的内容返回 `UNSUPPORTED_TEXT`。图片可用 `preview_file`；桌面编辑器另行读取完整原文。

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

`remove_path` 必须单独授权，不需要授予 `exec` 或关闭桌面删除保护。示例：

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

`list` 只传 `action`，不接受 `server_id`。读取返回 Base64 字节、下一 `cursor`、`alive` 和 `truncated`；后续读取使用上次返回的游标。MCP 打开终端和发送输入还需开启全局「允许 MCP 交互终端」。

## 权限与审计

12 项授权分别控制读取、预览、写入、编辑、安全删除、命令和六种终端动作。新服务器默认仅允许读取和预览；保存服务器时拒绝未知或退役权限名。全局只读与审批可以进一步限制已授予操作。

编辑审批展示替换请求前 8192 个字符；删除审批展示递归与缺失处理选项。完整请求绑定到单次审批。审计不保存直接文件正文、编辑原文、新文本或 diff，但会保存命令及其有界输出。完整策略见 [安全控制](SECURITY_CONTROLS.md)。
