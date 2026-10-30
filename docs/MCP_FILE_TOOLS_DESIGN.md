# MCP 文件工具设计：参考 Pi Agent

状态：已落地到 0.2.0，含后端、权限 UI 与自动化测试。日期：2026-10-31。

## 1. 源码依据与取舍

以 [earendil-works/pi](https://github.com/earendil-works/pi) 源码为依据，核对日期为 2026-10-31；本次实现记录的上游版本为 [2a46c4208004c4c5a41b75d775710f774ad9b630](https://github.com/earendil-works/pi/tree/2a46c4208004c4c5a41b75d775710f774ad9b630/packages/coding-agent/src/core/tools)。下表 main 链接便于跟踪后续变更。本文不以教程为依据。

| 源码                                                                                                                                 | 实际行为                                                           | 本项目设计                                              |
| ------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------ | ------------------------------------------------------- |
| [read.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/read.ts)                               | path、offset?、limit?；行号从 1 开始；原始文本附续读提示；支持图片 | 保留文本与分页语义；图片暂由 preview_file 处理          |
| [truncate.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/truncate.ts)                       | 默认最多 2000 行、50 KiB；从头截断保留完整行                       | 沿用输出预算，长行不返回假完整内容                      |
| [edit.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/edit.ts)                               | edits[]；去 BOM、归一化换行、写回恢复；生成 diff/patch             | 沿用批量替换及文本处理；字段使用项目的 snake_case       |
| [edit-diff.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/edit-diff.ts)                     | 精确匹配优先、有限模糊匹配回退；拒绝歧义、重叠和无变化             | 沿用匹配策略，增加明确错误码和输出预算                  |
| [write.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/write.ts)                             | path、content；创建或覆盖，自动创建父目录                          | 默认创建/覆盖；额外提供 create 模式；建目录继续独立授权 |
| [file-mutation-queue.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/file-mutation-queue.ts) | 模块级队列按文件路径串行；已有文件使用 realpath 作为 key           | 使用共享队列；第一版按服务器串行以覆盖目录移动冲突      |
| [ls.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/ls.ts)                                   | path?、limit?；默认 500 项；按名称排序，含隐藏文件，目录加 /       | 保留本项目条目元数据与目录优先排序；增加分页            |

独立 Schema、Operations 适配和读/写/编辑职责划分取自源码。create 模式、目录分页、SFTP 流式扫描、独立删除、严格参数校验、权限、审批和原子提交属于本项目设计，不应描述成 Pi 自带行为。Pi 的默认 write Operations 调用本地 writeFile，不能据此推断其拥有本项目要求的 SFTP 原子提交保证。

目标是让 Agent 用最少且准确的参数完成远端文件操作，读取大文件时控制上下文，修改已有文件时只提交差量。保留项目现有工具名称、SFTP 根目录、逐项授权、审批和审计体系。

Pi 本身没有独立删除工具；本项目的 remove 和 rename 是远程文件管理的扩展设计。

## 2. 当前问题

1. src-tauri/src/mcp.rs 的 15 个带参数工具共用 RemoteArgs，全部暴露 10 个字段，只有 server_id 必填。缺少 path、command 等字段会以空字符串进入业务逻辑。
2. read_file 返回整份文本，默认读取上限为 128 MiB；传输限制不适合作为 Agent 的上下文限制。
3. write_file 只有全文创建/覆盖，缺少 edit_file，Agent 小改动也必须提交整个文件。
4. list_files 没有分页或输出预算，大目录可能撑满上下文。
5. 原子重命名能避免半份文件，但当前操作间没有共享的文件修改队列，不能防止同一文件的并发读改写覆盖。

## 3. 工具集合与参数

保留现有 17 个工具，新增 edit_file，共 18 个。find_files、search_files 作为后续独立扩展，不在本轮核心增删改查接口内。

每个工具使用自己的参数结构。下面未标记 ? 的字段全部必填，? 表示可省略。所有对象拒绝未定义字段；必填字符串不得缺失，标识、路径和匹配文本不得为空。content 和 new_text 允许为空字符串。

| 工具         | 参数                             | 职责                         |
| ------------ | -------------------------------- | ---------------------------- |
| list_files   | server_id, path, offset?, limit? | 分页浏览目录                 |
| read_file    | server_id, path, offset?, limit? | 分段读取 UTF-8 文本          |
| write_file   | server_id, path, content, mode?  | 创建或完整覆盖文本           |
| edit_file    | server_id, path, edits           | 唯一文本匹配，批量差量修改   |
| remove       | server_id, path                  | 删除文件、符号链接或空目录   |
| mkdir        | server_id, path                  | 创建单级目录，父目录必须存在 |
| rename       | server_id, path, target          | 重命名/移动，不覆盖目标      |
| preview_file | server_id, path                  | 保留现有文本/图片预览        |
| download     | server_id, path                  | 保留 Base64 完整下载         |
| upload       | server_id, path, content         | 保留 Base64 完整上传         |

其余工具也拆分 Schema：exec 只接收 server_id、command、timeout_secs?；终端工具仅接收各自需要的会话、游标、尺寸或输入字段。list_servers 和 list_terminals 继续无参数。

路径继续要求远端 POSIX 绝对路径，且必须落在配置 root 内。暂不引入相对路径和 cwd，避免与持久终端的工作目录混淆。禁止 NUL、反斜杠、..，沿用 realpath 的根目录校验。不能使用运行在 Windows 上的本地路径规则解释远端路径。

## 4. read_file：分页、原始文本、预算

调用示例：

```json
{
  "server_id": "prod",
  "path": "/srv/app/main.rs",
  "offset": 1,
  "limit": 200
}
```

- offset：整数，minimum=1，默认 1，表示起始行号，包含该行。
- limit：整数，范围 1–2000，默认 2000，表示最多返回多少行。
- 正文最多 50 KiB，另预留最多 1 KiB 的续读提示。达到行数、字节或下载扫描预算任一上限即停止，不静默返回不完整内容。
- 与 Pi 源码一致，正文不添加行号前缀，保留缩进、CRLF/LF 和 UTF-8 内容；行号范围放在元数据和续读提示里，便于直接复制正文作为 old_text。
- 空文件成功返回空内容，start_line/end_line 为 null，has_more=false。末尾单个换行不额外制造一行；文本中的真实空行保留。
- 文件非空且 offset 超出 EOF 时返回 OFFSET_OUT_OF_RANGE，并返回已知的行数信息。
- 本项目使用有界缓冲流式扫描，不能先把整份文件读进内存再截断。这是 SFTP 场景的改进；Pi read.ts 实际先读取全文再分页。扫描字节不得超过当前 download_limit；命中扫描预算而无法定位 offset 时返回 READ_SCAN_LIMIT，建议下载或缩小 offset。
- 返回 total_lines=null，除非本次实际扫描到了 EOF。不得仅为计算总行数读取整个大文件。
- 非 UTF-8 或发现 NUL 时返回 UNSUPPORTED_TEXT，指引调用 preview_file/download。图片使用已有 preview_file，不把 Base64 塞入 read_file 的文本输出。

正常续读元数据示例：

```json
{
  "path": "/srv/app/main.rs",
  "start_line": 1,
  "end_line": 200,
  "returned_lines": 200,
  "has_more": true,
  "next_offset": 201,
  "total_lines": null,
  "truncated": true,
  "truncated_by": "requested_limit"
}
```

truncated_by 可为 requested_limit、output_bytes、scan_bytes；没有截断时为 null。扫描预算命中后，已完整返回的行可以使用 next_offset 续读，但不能保证下一次能在相同扫描预算内定位，提示中必须说明这一点。

普通字节截断只在完整行边界发生。若本次起始行本身超过输出预算，与 Pi truncateHead 一样不返回部分行，标明 first_line_exceeds_limit=true、next_offset=null、truncated_by=long_line，提示下载文件。Pi read.ts 的提示建议 bash；本项目使用 download，避免引入额外的 shell 权限依赖。避免返回相同 next_offset 导致 Agent 无限续读同一行。

MCP content 提供原始文本和明确续读提示；structuredContent 仅提供上述元数据，避免重复携带正文。这与 Pi 的 structuredContent 包含读取结果的形式不同，是本项目 MCP 返回契约的选择。桌面文件编辑接口仍读取原始全文，不套用 MCP 输出预算。

## 5. write_file：新增与全文覆盖

```json
{
  "server_id": "prod",
  "path": "/srv/app/config.json",
  "content": "{\n  \"port\": 8080\n}\n",
  "mode": "create"
}
```

- mode 枚举 create、overwrite，默认 overwrite，兼容现有创建或覆盖行为。
- create：目标已存在则返回 ALREADY_EXISTS，包括符号链接；禁止先检查存在性后再无条件覆盖。
- overwrite：不存在则创建，存在则完整替换。小范围修改优先使用 edit_file。
- content 是必填 UTF-8 字符串，空串表示创建/覆盖为空文件。
- 父目录必须存在；缺少时返回 PARENT_NOT_FOUND，Agent 先按已有 mkdir 权限逐级创建。
- 使用现有 upload_limit 约束 UTF-8 字节数；同目录临时文件、关闭写入后原子重命名，保留现有权限位。服务端不支持所需原子操作时失败，不降级为先清空目标再写入。
- create 提交使用 hardlink@openssh.com 将完整临时文件链接到尚不存在的目标，再清理临时名称；不支持时返回 ATOMIC_CREATE_UNSUPPORTED，不能把 flags=0 的行为直接视为跨服务端保证。overwrite 使用 posix-rename@openssh.com，不支持则返回 ATOMIC_WRITE_UNSUPPORTED。通过独立 SFTP 子系统通道发送扩展请求，不回退到通用重命名或 shell。
- 返回 path、created、written_bytes，并保留桌面已有 written 字段。create 成功必定为创建；overwrite 的 created 基于提交前 stat，在应用队列内准确，但无法判断外部进程在检查与提交之间的创建/删除。

## 6. edit_file：精确、批量、一次提交

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

- edits 是必填数组，1–100 项。每项仅包含必填 old_text、new_text。
- old_text 非空，优先精确匹配；仍按 Pi 的归一化文本空间检查唯一性，即使原始精确匹配只有一处，归一化后多处相同也返回 EDIT_AMBIGUOUS。无法匹配时返回 EDIT_NOT_FOUND，提示增加上下文或重新读取。
- new_text 允许为空，用于删除匹配片段；追加内容通过匹配文件尾部的唯一文本完成，不使用空 old_text。
- 所有项针对同一个原始文件匹配，拒绝重复、嵌套、相互重叠的区间；返回 EDIT_OVERLAP。
- 全部验证成功后按位置倒序替换，再原子写入一次。任何一项失败都不提交文件，不能按数组顺序边匹配边落盘。
- 匹配前剥离 BOM，并将文件和替换参数的换行归一化为 LF；精确匹配失败时按源码回退到 NFKC、行尾空白、智能引号、Unicode 横线和特殊空格归一化后的匹配。不得扩大为任意编辑距离搜索。
- 各编辑项使用同一匹配空间。如果任一项需要模糊匹配，则全部项在该空间重新匹配并检查唯一性和重叠。归一化后 old_text 为空也必须拒绝，避免匹配所有位置。
- 模糊匹配只回写发生变化的行块，不能把全文的引号、空白等一起规范化；保留未修改行内容。提交时恢复 BOM 和检测到的换行样式。与 Pi 一样，混合换行可能被统一为检测到的样式，不能承诺全文未修改字节完全不变。
- 最终内容无变化时返回 EDIT_NO_CHANGE，不重写文件，与源码行为一致。
- 原文件须为普通 UTF-8 文本，无 NUL；原文件受 download_limit 约束，最终文件和替换请求总字节分别受 upload_limit 约束。先校验再分配最终缓冲区。
- 返回 path、changed、applied_edits、first_changed_line、written_bytes，以及最多 50 KiB 的统一 diff；diff 截断必须标记 diff_truncated。正文和 diff 不写入持久审计。

## 7. remove 与 rename

remove 不新增 recursive 或 force 参数。删除目录仅允许空目录；非空返回 DIRECTORY_NOT_EMPTY；不存在返回 NOT_FOUND。删除符号链接作用于链接本身；保留现有保守边界规则，对链接解析越界或悬空的情况拒绝操作。授权根目录不可删除或重命名。

rename 的源和目标都必须在 root 内，目标存在则 ALREADY_EXISTS；父目录必须存在。源若为链接，则移动链接本身。保留当前不覆盖目标的约定，并在后端验证实际服务端的 no-replace 行为；无法保证时明确失败。

remove 返回 path、removed=true、kind=file/directory/symlink。rename 返回 path、target、renamed=true。

删除仍受项目的删除保护、只读模式和审批策略约束。不要为“方便 Agent”把删除转换为 exec rm，也不要在 SFTP 失败后自动改用 shell。

## 8. list_files：有界浏览

- path 必填；offset 为从 0 开始的条目索引，默认 0；limit 范围 1–1000，默认 200。描述明确区分条目索引与 read_file 的行号。
- 保留目录优先、同类型按名称排序。返回 path、entries、has_more、next_offset、truncated、truncated_by。
- 返回内容最多 50 KiB；只包含完整条目。单个条目超限则返回 ENTRY_TOO_LARGE，不能给出不前进的游标。
- offset 基于当前目录快照，不保证并发增删时无遗漏或重复；目录变动后重新从 0 浏览。
- 实现使用 opendir/readdir 迭代并设置 10000 条扫描上限。为了保持排序语义，扫描超限时返回 DIRECTORY_SCAN_LIMIT，提示后续 find_files 或缩小目录；不能把未扫描完的目录伪装为完整排序结果。

## 9. Schema、执行与权限

建议边界：

```text
工具专属输入 Schema
  -> 解析为类型化请求
  -> 当前服务器授权 / 全局策略 / 审批
  -> 解析远端路径并校验 root
  -> 共享修改队列
  -> SFTP Operations 适配层
  -> MCP 文本、结构化元数据 / 安全审计摘要
```

- 用 PathArgs、ReadFileArgs、WriteFileArgs、EditFileArgs、RenameArgs 等替换 MCP 层 RemoteArgs。Schema 和 Rust 反序列化都拒绝额外字段；数值范围同时做运行时校验。
- 内部新增类型化文件请求；不把 edits 序列化进 content，不把行号挤进 cursor。桌面旧 Operation 通过适配层调用同一文件服务。
- 分离文件逻辑和 SFTP：读取分页、匹配替换、输出截断可以用内存 Operations 验证；真实路径校验和提交由 SFTP 实现负责。
- 新增 edit_file 权限，要求同时具备 read_file 和 edit_file。它具有读取、匹配和生成 diff 的能力，单独 write_file 授权不隐含 edit_file。
- 老配置不自动获得新增权限；权限清单、前端勾选、数据库校验及 list_servers 同步支持 edit_file，默认关闭。
- edit_file 必须进入敏感操作判定、只读拦截和审批链。审批绑定 server_id、路径、全部替换内容及当前服务器配置；审批后重新检查授权和策略。
- 审计只保存路径、编辑数量、成功/失败、耗时、写入字节和截断标志。错误消息不得回显文件片段；新增 edit_file 的 diff 不进入通用 audit_response 分支，避免落盘。
- MCP 错误通过 isError=true 返回稳定 code、message、可选的恢复建议；未知字段、缺失字段等协议参数错误由框架返回。文件内容不得出现在错误日志中。

## 10. 并发与提交边界

write_file、edit_file、upload、remove、rename、mkdir 共用应用级修改队列，桌面与 MCP 都进入该队列。锁覆盖路径复查、读取原文、验证、生成结果和提交，不能只包住最后的 write。

建议第一版按 server_id 串行化全部文件修改，正确处理目录 rename/remove 与子文件操作，代价是同一服务器的独立文件也会等待。后续确有吞吐需求再升级为能处理祖先/后代冲突的路径锁；简单的逐文件锁无法覆盖目录移动。

等待用户审批时不持有修改锁；获批、获取队列后重新检查权限和路径。队列在取消时必须等正在执行的 SFTP 调用结束才释放，避免已返回取消的写入继续与下一次操作并发。网络错误发生在提交之后但响应之前时，返回 COMMIT_STATUS_UNKNOWN，提示先读回核验，不能自动重试编辑/删除。

该队列只协调本应用内请求，不能锁住服务器上的外部进程。原子重命名保证文件完整性，不提供跨进程 compare-and-swap 或持久化 fsync 保证。第一版不宣称防止外部并发覆盖；需要强并发控制时另设计服务端事务能力。

## 11. 兼容与实施顺序

这是新版本的接口契约，不能直接声称完全向后兼容：read_file 从完整文本 JSON 改为有界原始文本及续读元数据，list_files 增加分页，额外字段也将从忽略变为拒绝。现有只提交实际所需字段的写入/删除调用保持参数兼容；write_file 默认 overwrite 保留原有行为。

实施时更新 MCP server version、README 工具表、客户端说明和集成测试。客户端需重新连接/发现工具，不能继续把 read_file 的第一个文本块当作旧 JSON 全文解析；完整文件传输使用 download。桌面原始文本编辑流程保持独立适配，不因 MCP 分页改变。

1. 拆分所有工具 Schema，完成必填字段、范围与未知字段校验。
2. 实现 read_file、list_files 的有界输出和续读协议。
3. 引入共享文件服务/修改队列，增加 edit_file 和 write_file mode。
4. 接入新增权限、只读/审批/审计，更新前端操作标签和文档。
5. 完成协议与真实 SFTP 验证后再发布新的服务端版本。

必要验证：Schema 不暴露无关字段；缺失路径/命令直接失败；中文和 UTF-8 字节边界；CRLF/BOM/末尾空行；空文件；越界 offset；超长单行；大目录和输出上限；编辑零匹配/重复/重叠/批量失败不落盘；精确优先、模糊回退、归一化歧义、归一化后空匹配、未变化行保持、无变化拒绝；create 竞争；同应用并发编辑；目录移动与子文件写入；越界与符号链接；只读、删除保护、撤销权限、审批后变更；日志中无正文/diff；取消、重命名失败时旧文件完整；提交状态未知不自动重试。

scripts/integration.mjs 已更新为发现 18 个工具并读取原始文本/structuredContent，覆盖原子写入、仅创建竞争、扩展不支持、提交响应丢失、并发编辑和日志脱敏。这些测试使用内存 SSH/SFTP 服务，不能替代针对真实目标 OpenSSH 服务端的验证。
