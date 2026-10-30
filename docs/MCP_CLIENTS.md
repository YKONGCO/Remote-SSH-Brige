# MCP 客户端接入

MCP 页面提供 Claude Code、Codex、Gemini CLI、OpenCode、Cursor、Pi 的用户级一键配置。先启动本地服务，再点击对应客户端的一键配置；修改已有文件前会在同目录创建 `.remote-desk-<uuid>.bak` 备份。客户端内需重新连接或重载服务，Pi 使用 `/reload`。

只更新 `remote-desk` 条目，保留其他服务和无关设置。JSON/JSONC 使用语法树编辑，Codex TOML 使用 `toml_edit`，保留无关注释。损坏的文件、错误类型的服务映射、非普通文件和超过 2 MiB 的配置会停止自动写入。写入通过临时文件替换，同时检查外部修改并防止重复写入。

服务每次启动会更换令牌，已配置客户端会显示“需要更新”。检测只判断配置是否匹配，不代表客户端已运行、已连接或已安装。浏览器演示不写入本机配置。

页面的手动配置区直接显示通用 `mcpServers` JSON 并提供复制按钮。一键配置仍按目标客户端的原生格式写入。

配置路径：

| 客户端      | 默认用户配置                                                                  |
| ----------- | ----------------------------------------------------------------------------- |
| Claude Code | `~/.claude.json`                                                              |
| Codex       | `~/.codex/config.toml`，支持绝对路径的 `CODEX_HOME`                           |
| Gemini CLI  | `~/.gemini/settings.json`                                                     |
| OpenCode    | `~/.config/opencode/opencode.json`，优先已有 `.jsonc`，支持 `XDG_CONFIG_HOME` |
| Cursor      | `~/.cursor/mcp.json`                                                          |
| Pi          | `~/.pi/agent/mcp.json`，支持绝对路径的 `PI_CODING_AGENT_DIR`                  |

配置格式依据：[Claude Code MCP](https://code.claude.com/docs/en/mcp)、[Codex MCP](https://developers.openai.com/codex/mcp)、[Gemini CLI MCP](https://geminicli.com/docs/tools/mcp-server/)、[OpenCode MCP](https://opencode.ai/docs/mcp-servers/)、[Cursor MCP](https://cursor.com/docs/context/mcp)、[Pi MCP](https://pi.dev/docs/latest/mcp)。

视觉参考 [Windows 设计指南](https://learn.microsoft.com/zh-cn/windows/apps/design/guidelines-overview)，统一中性背景、少量强调色、4px 控件圆角与 8px 容器/弹层圆角。客户端图标来自 [Lobe Icons](https://github.com/lobehub/lobe-icons)，MIT 许可保存在 `src/assets/clients/LICENSE`。
