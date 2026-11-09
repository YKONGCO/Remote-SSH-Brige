import { api, desktop } from "../api";
import type {
  CommandPolicy,
  RuntimeSettings,
  NumericRuntimeSetting,
} from "../runtime-settings";
import {
  defaultTransferSettings,
  effectiveCommandPolicy,
  commandMode,
  advancedShell,
  defaultAllowedCommands,
  validCommandLists,
  parseCommandList,
  MAX_COMMAND_RULES,
} from "../runtime-settings";
import { CommandPresets } from "./CommandPresets";
import { X, RotateCcw, Check, ShieldBan, FolderOpen } from "lucide-react";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";
import { Textarea } from "./ui/textarea";
import { SelectField } from "./SelectField";
import { TransferLimitField } from "./TransferLimitField";
import { useEffect, useState, type ReactNode } from "react";
import {
  defaults,
  savePreferences,
  usePreferences,
  type Preferences,
} from "../preferences";

function Row({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <div className="setting-row">
      <div>
        <strong>{title}</strong>
        <p>{description}</p>
      </div>
      {children}
    </div>
  );
}
export function Settings() {
  const preferences = usePreferences();
  const [error, setError] = useState("");
  const [runtime, setRuntime] = useState<RuntimeSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [listsOpen, setListsOpen] = useState(false);
  const [rootsEditor, setRootsEditor] = useState<
    "local_upload_roots" | "local_download_roots" | null
  >(null);
  const [rootsDraft, setRootsDraft] = useState("");
  const [pickingDirectories, setPickingDirectories] = useState(false);
  const [allowedDraft, setAllowedDraft] = useState("");
  const [blockedDraft, setBlockedDraft] = useState("");
  const policy = runtime ? effectiveCommandPolicy(runtime) : undefined;
  async function pickDirectories() {
    if (pickingDirectories || saving) return;
    setPickingDirectories(true);
    setError("");
    try {
      const selected = await api<string[]>("pick_local_directories");
      if (!selected.length) return;
      const paths = rootsDraft
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter(Boolean);
      const key = (path: string) => {
        const windows = /^[a-z]:[\\/]|^\\\\/i.test(path);
        const normalized = windows
          ? path.replaceAll("\\", "/").toLowerCase()
          : path;
        return normalized.replace(/\/+$/, "") || "/";
      };
      const seen = new Set(paths.map(key));
      for (const path of selected) {
        if (!seen.has(key(path))) {
          paths.push(path);
          seen.add(key(path));
        }
      }
      if (paths.length > 64) {
        setError("最多授权 64 个目录，请先移除部分目录。");
        return;
      }
      setRootsDraft(paths.join("\n"));
    } catch (e) {
      setError(String(e));
    } finally {
      setPickingDirectories(false);
    }
  }
  async function updateCommand(patch: Partial<CommandPolicy>) {
    if (!policy) return false;
    const next = {
      ...policy,
      execution_mode: commandMode(policy),
      allowed_commands: policy.allowed_commands ?? [...defaultAllowedCommands],
      blocked_commands: policy.blocked_commands ?? [],
      ...patch,
    };
    if (next.allow_advanced_shell !== undefined) {
      next.allow_pipes =
        next.allow_redirection =
        next.allow_compound =
        next.allow_scripts =
        next.allow_substitution =
          next.allow_advanced_shell;
    }
    next.diagnostic_only = next.execution_mode === "whitelist";
    return updateRuntime("command_policy", next);
  }
  async function loadRuntime() {
    try {
      setRuntime(await api<RuntimeSettings>("runtime_settings"));
      setLoadFailed(false);
      setError("");
    } catch (e) {
      setError(String(e));
      setLoadFailed(true);
    }
  }
  useEffect(() => {
    void loadRuntime();
  }, []);
  async function updateRuntime<K extends keyof RuntimeSettings>(
    name: K,
    value: RuntimeSettings[K],
  ) {
    if (!runtime || saving) return false;
    const previous = runtime;
    const next = {
      ...runtime,
      command_policy: effectiveCommandPolicy(runtime),
      [name]: value,
    };
    setRuntime(next);
    setSaving(true);
    try {
      setRuntime(
        await api<RuntimeSettings>("save_runtime_settings", {
          settings: next,
        }),
      );
      setError("");
      return true;
    } catch (e) {
      setRuntime(previous);
      setError(String(e));
      return false;
    } finally {
      setSaving(false);
    }
  }
  function runtimeSelect(
    name: NumericRuntimeSetting,
    label: string,
    values: number[],
    unit: string,
  ) {
    const options = [
      ...new Set([...values, ...(runtime ? [runtime[name]] : [])]),
    ].sort((a, b) => a - b);
    return (
      <SelectField
        className="setting-select"
        aria-label={label}
        disabled={!runtime || saving}
        value={runtime ? String(runtime[name]) : ""}
        onValueChange={(value) => void updateRuntime(name, Number(value))}
      >
        {!runtime && (
          <option value="">{loadFailed ? "加载失败" : "加载中…"}</option>
        )}
        {options.map((n) => (
          <option key={n} value={n}>
            {n} {unit}
          </option>
        ))}
      </SelectField>
    );
  }
  function transferSelect(
    name: keyof typeof defaultTransferSettings,
    label: string,
    values: number[],
    unit: string,
  ) {
    const current = runtime?.transfers?.[name] ?? defaultTransferSettings[name];
    return (
      <SelectField
        className="setting-select"
        aria-label={label}
        disabled={!runtime || saving}
        value={String(current)}
        onValueChange={(value) =>
          void updateRuntime("transfers", {
            ...defaultTransferSettings,
            ...runtime?.transfers,
            [name]: Number(value),
          })
        }
      >
        {[...new Set([...values, current])]
          .sort((a, b) => a - b)
          .map((n) => (
            <option key={n} value={n}>
              {n} {unit}
            </option>
          ))}
      </SelectField>
    );
  }
  function update(patch: Partial<Preferences>) {
    try {
      savePreferences({ ...preferences, ...patch });
      setError("");
    } catch {
      setError("设置保存失败，请检查本地存储空间后重试。");
    }
  }
  function select(
    name: keyof Preferences,
    label: string,
    options: [string, string][],
  ) {
    return (
      <SelectField
        className="setting-select"
        aria-label={label}
        value={String(preferences[name])}
        onValueChange={(value) => update({ [name]: value })}
      >
        {options.map(([value, text]) => (
          <option key={value} value={value}>
            {text}
          </option>
        ))}
      </SelectField>
    );
  }
  function toggle(
    name: "motion" | "showHidden" | "cursorBlink",
    label: string,
  ) {
    return (
      <button
        type="button"
        className="setting-switch"
        role="switch"
        aria-label={label}
        aria-checked={preferences[name]}
        onClick={() => update({ [name]: !preferences[name] })}
      >
        <span />
      </button>
    );
  }
  function runtimeToggle(
    name:
      | "deletion_protection"
      | "block_dangerous_commands"
      | "remote_read_only"
      | "mcp_auto_start",
    label: string,
  ) {
    return (
      <button
        type="button"
        className="setting-switch"
        role="switch"
        aria-label={label}
        disabled={!runtime || saving}
        aria-checked={runtime?.[name] ?? false}
        onClick={() => void updateRuntime(name, !runtime?.[name])}
      >
        <span />
      </button>
    );
  }
  function commandToggle(
    name: "allow_advanced_shell" | "allow_mcp_terminal",
    label: string,
  ) {
    const checked = policy
      ? name === "allow_advanced_shell"
        ? advancedShell(policy)
        : policy.allow_mcp_terminal
      : false;
    return (
      <button
        type="button"
        className="setting-switch"
        role="switch"
        aria-label={label}
        aria-checked={checked}
        disabled={!runtime || saving}
        onClick={() =>
          policy &&
          void updateCommand({
            [name]: !checked,
          })
        }
      >
        <span />
      </button>
    );
  }
  return (
    <div className="settings-layout" aria-busy={saving}>
      {error && !listsOpen && !rootsEditor && (
        <p role="alert" className="error-inline">
          {error}{" "}
          {loadFailed && (
            <button className="text-btn" onClick={() => void loadRuntime()}>
              重试
            </button>
          )}
        </p>
      )}
      <section className="settings-section">
        <h2 className="settings-group-title">常规</h2>
        <div className="settings-card">
          <Row title="启动页面" description="打开应用时显示的页面。">
            {select("startPage", "启动页面", [
              ["servers", "服务器"],
              ["files", "远程文件"],
              ["terminals", "终端会话"],
              ["jobs", "后台任务"],
              ["tunnels", "端口转发"],
              ["audit", "操作记录"],
              ["mcp", "MCP 服务"],
              ["settings", "设置"],
            ])}
          </Row>
          <Row title="关闭窗口时" description="收起到托盘可保持后台运行。">
            <SelectField
              className="setting-select"
              aria-label="关闭窗口时"
              disabled={!runtime || saving}
              value={runtime ? (runtime.close_to_tray ? "tray" : "exit") : ""}
              onValueChange={(value) =>
                void updateRuntime("close_to_tray", value === "tray")
              }
            >
              {!runtime && (
                <option value="">{loadFailed ? "加载失败" : "加载中…"}</option>
              )}
              <option value="tray">最小化到托盘</option>
              <option value="exit">直接退出</option>
            </SelectField>
          </Row>
          <Row
            title="自动启动 MCP 服务"
            description="下次打开应用时按上次成功启动的地址和端口启动服务，并更新已接入客户端的配置（自动备份）。"
          >
            {runtimeToggle("mcp_auto_start", "自动启动 MCP 服务")}
          </Row>
          <Row
            title="状态刷新频率"
            description="列表自动刷新间隔，不影响终端输出。"
          >
            {select("refreshSeconds", "状态刷新频率", [
              ["3", "每 3 秒"],
              ["5", "每 5 秒"],
              ["10", "每 10 秒"],
              ["30", "每 30 秒"],
              ["0", "仅手动刷新"],
            ])}
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">外观与体验</h2>
        <div className="settings-card">
          <Row title="主题外观" description="浅色、深色或跟随系统。">
            {select("theme", "主题外观", [
              ["light", "浅色"],
              ["dark", "深色"],
              ["system", "跟随系统"],
            ])}
          </Row>
          <Row title="界面密度" description="调整列表行距。">
            {select("density", "界面密度", [
              ["comfortable", "舒适"],
              ["compact", "紧凑"],
            ])}
          </Row>
          <Row title="界面动效" description="按钮与面板的过渡效果。">
            {toggle("motion", "界面动效")}
          </Row>
        </div>
      </section>
      <section className="settings-section" id="security-settings">
        <h2 className="settings-group-title">安全控制</h2>
        <div className="settings-card">
          <Row
            title="只读保护"
            description="禁止文件修改、命令和终端交互，允许查看与下载。"
          >
            {runtimeToggle("remote_read_only", "只读保护")}
          </Row>
          <Row
            title="删除保护"
            description="禁止桌面和 MCP 文件删除，不限制 Shell 和终端。"
          >
            {runtimeToggle("deletion_protection", "删除保护")}
          </Row>
          <Row
            title="危险命令拦截"
            description="拦截 rm、磁盘破坏、关机等高危命令。"
          >
            {runtimeToggle("block_dangerous_commands", "危险命令拦截")}
          </Row>
          <Row
            title="审批模式"
            description="敏感操作需确认，超过等待时间则自动拒绝。"
          >
            <SelectField
              className="setting-select"
              aria-label="审批模式"
              disabled={!runtime || saving}
              value={runtime?.approval_mode || "off"}
              onValueChange={(value) =>
                void updateRuntime(
                  "approval_mode",
                  value as RuntimeSettings["approval_mode"],
                )
              }
            >
              <option value="off">关闭审批</option>
              <option value="mcp">仅 MCP 敏感操作</option>
              <option value="all">所有来源的敏感操作</option>
            </SelectField>
          </Row>
          <Row
            title="审批等待时间"
            description="超时自动拒绝。修改对新请求生效，待审批请求保留原截止时间。"
          >
            {runtimeSelect(
              "approval_timeout_secs",
              "审批等待时间",
              [1, 5, 10, 15, 30, 60, 120],
              "秒",
            )}
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">命令与 Shell 权限</h2>
        <div className="settings-card">
          <Row
            title="命令执行范围"
            description="“不限命令”仍受黑名单和高危拦截限制。"
          >
            <SelectField
              className="setting-select"
              aria-label="命令执行范围"
              disabled={!runtime || saving}
              value={policy ? commandMode(policy) : "whitelist"}
              onValueChange={(execution_mode) =>
                void updateCommand({
                  execution_mode:
                    execution_mode as CommandPolicy["execution_mode"],
                })
              }
            >
              <option value="disabled">禁止执行</option>
              <option value="whitelist">仅允许白名单</option>
              <option value="unrestricted">不限命令</option>
            </SelectField>
          </Row>
          <Row
            title="命令名单管理"
            description="配置白名单和黑名单，黑名单优先。"
          >
            <Button
              variant="outline"
              size="sm"
              disabled={!runtime || saving}
              onClick={() => {
                setAllowedDraft(
                  (policy?.allowed_commands ?? defaultAllowedCommands).join(
                    "\n",
                  ),
                );
                setBlockedDraft((policy?.blocked_commands ?? []).join("\n"));
                setListsOpen(true);
                setError("");
              }}
            >
              管理名单
            </Button>
          </Row>
          <Row
            title="允许 Shell 高级功能"
            description="允许管道、重定向、组合命令与脚本；脚本内容无法完整检查。"
          >
            <div className="flex items-center gap-2">
              {policy &&
                policy.allow_advanced_shell === undefined &&
                advancedShell(policy) &&
                ![
                  policy.allow_pipes,
                  policy.allow_redirection,
                  policy.allow_compound,
                  policy.allow_scripts,
                  policy.allow_substitution,
                ].every(Boolean) && (
                  <span className="muted">旧配置：部分开启</span>
                )}
              {commandToggle("allow_advanced_shell", "允许 Shell 高级功能")}
            </div>
          </Row>
          <Row
            title="允许 AI 交互终端"
            description="需服务器授权；终端输入不受命令名单检查。"
          >
            {commandToggle("allow_mcp_terminal", "允许 AI 交互终端")}
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">连接与命令</h2>
        <div className="settings-card">
          <Row title="连接超时" description="建立服务器连接的最长等待时间。">
            {runtimeSelect(
              "connect_timeout_secs",
              "连接超时",
              [3, 8, 15, 30, 60, 120],
              "秒",
            )}
          </Row>
          <Row
            title="握手超时"
            description="等待 SSH 握手与服务器横幅的最长时间。"
          >
            {runtimeSelect(
              "handshake_timeout_secs",
              "握手超时",
              [5, 15, 30, 60, 120],
              "秒",
            )}
          </Row>
          <Row
            title="读写超时"
            description="普通 SSH 操作的单次等待上限，文件传输使用下方独立设置。"
          >
            {runtimeSelect(
              "io_timeout_secs",
              "读写超时",
              [5, 15, 30, 60, 120, 300],
              "秒",
            )}
          </Row>
          <Row
            title="默认命令超时"
            description="MCP 命令的默认时限，单次可覆盖，最长 300 秒。"
          >
            {runtimeSelect(
              "command_timeout_secs",
              "默认命令超时",
              [10, 30, 60, 120, 300],
              "秒",
            )}
          </Row>
          <Row
            title="命令超时后保留为后台任务"
            description="超时后继续运行，需服务器授权后台任务。"
          >
            <button
              type="button"
              className="setting-switch"
              role="switch"
              aria-label="命令超时后保留为后台任务"
              aria-checked={runtime?.exec_timeout_to_job ?? false}
              disabled={!runtime || saving}
              onClick={() =>
                void updateRuntime(
                  "exec_timeout_to_job",
                  !runtime?.exec_timeout_to_job,
                )
              }
            >
              <span />
            </button>
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">远程文件</h2>
        <div className="settings-card">
          <Row title="同时传输任务数" description="其余任务自动排队。">
            {transferSelect(
              "max_concurrent",
              "同时传输任务数",
              [1, 2, 4, 8, 16],
              "个",
            )}
          </Row>
          <Row
            title="每台服务器并发"
            description="同一主机的别名共享并发限制。"
          >
            {transferSelect(
              "per_server_concurrent",
              "每台服务器并发",
              [1, 2, 4, 8],
              "个",
            )}
          </Row>
          <Row
            title="自动重试次数"
            description="网络中断后退避重试；权限或文件变化错误会停止。"
          >
            {transferSelect(
              "max_retries",
              "自动重试次数",
              [0, 1, 3, 5, 10],
              "次",
            )}
          </Row>
          <Row title="传输 I/O 超时" description="单次读写等待时间。">
            {transferSelect(
              "io_timeout_secs",
              "传输 I/O 超时",
              [5, 15, 30, 60, 120, 300],
              "秒",
            )}
          </Row>
          <Row
            title="传输无进度超时"
            description="持续有进展的大文件传输不受总耗时限制。"
          >
            {transferSelect(
              "idle_timeout_secs",
              "传输无进度超时",
              [15, 30, 60, 120, 300, 600],
              "秒",
            )}
          </Row>
          <Row
            title="默认传输限速"
            description="每个任务的默认速度上限，0 表示不限。"
          >
            {transferSelect(
              "bandwidth_kib",
              "默认传输限速",
              [0, 128, 512, 1024, 4096, 16384],
              "KiB/s",
            )}
          </Row>
          <Row title="显示隐藏文件" description="显示 .ssh 等以点开头的文件。">
            {toggle("showHidden", "显示隐藏文件")}
          </Row>
          <Row
            title="单文件上传上限"
            description="单个上传文件的大小上限，0 表示不限。"
          >
            <TransferLimitField
              label="单文件上传上限"
              value={runtime?.upload_limit_mib}
              disabled={!runtime || saving}
              onSave={(value) => updateRuntime("upload_limit_mib", value)}
            />
          </Row>
          <Row
            title="单文件下载上限"
            description="单个下载文件的大小上限，0 表示不限。"
          >
            <TransferLimitField
              label="单文件下载上限"
              value={runtime?.download_limit_mib}
              disabled={!runtime || saving}
              onSave={(value) => updateRuntime("download_limit_mib", value)}
            />
          </Row>
          <Row
            title="MCP 本地上传目录"
            description="AI 可上传的本机目录，留空禁用。"
          >
            <div className="transfer-roots-summary">
              <span className="muted">
                {runtime?.local_upload_roots?.length
                  ? `已授权 ${runtime.local_upload_roots.length} 个目录`
                  : "未开放"}
              </span>
              <Button
                variant="outline"
                size="sm"
                aria-label="管理 MCP 本地上传目录"
                disabled={!runtime || saving}
                onClick={() => {
                  setRootsDraft((runtime?.local_upload_roots ?? []).join("\n"));
                  setRootsEditor("local_upload_roots");
                  setError("");
                }}
              >
                管理目录
              </Button>
            </div>
          </Row>
          <Row
            title="MCP 本地下载目录"
            description="AI 下载的本机保存目录，留空禁用。"
          >
            <div className="transfer-roots-summary">
              <span className="muted">
                {runtime?.local_download_roots?.length
                  ? `已授权 ${runtime.local_download_roots.length} 个目录`
                  : "未开放"}
              </span>
              <Button
                variant="outline"
                size="sm"
                aria-label="管理 MCP 本地下载目录"
                disabled={!runtime || saving}
                onClick={() => {
                  setRootsDraft(
                    (runtime?.local_download_roots ?? []).join("\n"),
                  );
                  setRootsEditor("local_download_roots");
                  setError("");
                }}
              >
                管理目录
              </Button>
            </div>
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">终端</h2>
        <div className="settings-card">
          <Row title="终端配色" description="独立于界面主题，默认深色。">
            {select("terminalTheme", "终端配色", [
              ["dark", "深色"],
              ["light", "浅色"],
              ["app", "跟随界面"],
            ])}
          </Row>
          <Row title="终端字号" description="调整终端文字大小。">
            {select(
              "terminalFontSize",
              "终端字号",
              [12, 13, 14, 15, 16].map((n) => [String(n), `${n} px`]),
            )}
          </Row>
          <Row title="光标闪烁" description="输入光标的闪烁效果。">
            {toggle("cursorBlink", "光标闪烁")}
          </Row>
          <Row
            title="回滚行数"
            description="界面保留的历史行数，不影响后端输出缓存。"
          >
            {select(
              "terminalScrollback",
              "回滚行数",
              [1000, 5000, 10000, 20000].map((n) => [
                String(n),
                `${n.toLocaleString()} 行`,
              ]),
            )}
          </Row>
          <Row
            title="终端会话上限"
            description="限制新建会话，不关闭已有会话。"
          >
            {runtimeSelect(
              "max_terminals",
              "终端会话上限",
              [1, 4, 8, 16, 32, 64],
              "个",
            )}
          </Row>
        </div>
      </section>
      {rootsEditor && (
        <ModalFrame
          className="settings-editor-dialog directory-roots-modal"
          width={640}
          onClose={() => {
            if (!saving && !pickingDirectories) {
              setRootsEditor(null);
              setError("");
            }
          }}
        >
          <form
            className="modal-layout"
            onSubmit={async (event) => {
              event.preventDefault();
              if (pickingDirectories) return;
              const paths = [
                ...new Set(
                  rootsDraft
                    .split(/\r?\n/)
                    .map((line) => line.trim())
                    .filter(Boolean),
                ),
              ];
              if (await updateRuntime(rootsEditor, paths)) setRootsEditor(null);
            }}
          >
            <div className="modal-head settings-editor-head">
              <DialogTitle>
                {rootsEditor === "local_upload_roots"
                  ? "MCP 本地上传目录"
                  : "MCP 本地下载目录"}
              </DialogTitle>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label="关闭目录管理"
                disabled={saving || pickingDirectories}
                onClick={() => {
                  setRootsEditor(null);
                  setError("");
                }}
              >
                <X size={16} />
              </Button>
            </div>
            <div className="modal-body">
              <div className="directory-roots-toolbar">
                <label htmlFor="local-roots-editor">
                  授权目录{" "}
                  <span>
                    {
                      new Set(
                        rootsDraft
                          .split(/\r?\n/)
                          .map((line) => line.trim())
                          .filter(Boolean),
                      ).size
                    }{" "}
                    / 64
                  </span>
                </label>
                <Button
                  type="button"
                  variant="outline"
                  disabled={!desktop || saving || pickingDirectories}
                  title={
                    desktop
                      ? "可一次选择多个文件夹"
                      : "仅桌面端支持选择，浏览器请手动填写"
                  }
                  onClick={() => void pickDirectories()}
                >
                  <FolderOpen size={14} />
                  {pickingDirectories ? "选择中…" : "选择文件夹"}
                </Button>
              </div>
              <Textarea
                id="local-roots-editor"
                autoFocus
                className="mono"
                aria-label="本地授权目录"
                rows={12}
                placeholder={
                  "每行一个本地绝对路径，留空禁用\n例如 D:/files 或 /home/user/files"
                }
                title="目录位于运行此应用的电脑，保存后生效"
                value={rootsDraft}
                disabled={saving || pickingDirectories}
                onChange={(event) => setRootsDraft(event.target.value)}
              />
              {error && (
                <p className="error-inline" role="alert">
                  {error}
                </p>
              )}
            </div>
            <div className="modal-foot flex gap-2 justify-end">
              <Button
                type="button"
                variant="ghost"
                className="settings-editor-reset"
                disabled={saving || pickingDirectories}
                onClick={() => {
                  setRootsDraft("");
                  setError("");
                }}
              >
                清空目录
              </Button>
              <Button
                type="button"
                variant="outline"
                disabled={saving || pickingDirectories}
                onClick={() => {
                  setRootsEditor(null);
                  setError("");
                }}
              >
                取消
              </Button>
              <Button type="submit" disabled={saving || pickingDirectories}>
                {saving ? "保存中…" : "保存目录"}
              </Button>
            </div>
          </form>
        </ModalFrame>
      )}
      {listsOpen && (
        <ModalFrame
          className="settings-editor-dialog command-list-modal"
          onClose={() => {
            if (!saving) {
              setListsOpen(false);
              setError("");
            }
          }}
          width={940}
        >
          <form
            className="modal-layout"
            onSubmit={async (event) => {
              event.preventDefault();
              const allowed_commands = parseCommandList(allowedDraft),
                blocked_commands = parseCommandList(blockedDraft);
              if (!validCommandLists(allowed_commands, blocked_commands)) {
                setError(
                  `每份名单最多 ${MAX_COMMAND_RULES} 项；不要填写参数、通配符或 Shell 语法。`,
                );
                return;
              }
              if (await updateCommand({ allowed_commands, blocked_commands }))
                setListsOpen(false);
            }}
          >
            <div className="modal-head settings-editor-head">
              <DialogTitle>命令名单管理</DialogTitle>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="command-modal-close"
                aria-label="关闭命令名单管理"
                disabled={saving}
                onClick={() => {
                  setListsOpen(false);
                  setError("");
                }}
              >
                <X size={18} />
              </Button>
            </div>
            <div className="modal-body">
              <CommandPresets
                value={allowedDraft}
                disabled={saving}
                onChange={setAllowedDraft}
                onError={setError}
              />
              <div
                className="command-list-editors"
                title="拖动右下角可调整名单输入区高度"
              >
                <label>
                  <span className="command-editor-label">
                    <span>
                      <Check size={14} />
                      允许执行
                    </span>
                    <small>
                      {parseCommandList(allowedDraft).length}
                      <em> / {MAX_COMMAND_RULES}</em>
                    </small>
                  </span>
                  <Textarea
                    aria-label="命令白名单"
                    className="mono"
                    rows={10}
                    placeholder="命令名或绝对路径，如 python3、/opt/venv/bin/python3"
                    value={allowedDraft}
                    disabled={saving}
                    onChange={(event) => setAllowedDraft(event.target.value)}
                  />
                </label>
                <label>
                  <span className="command-editor-label">
                    <span>
                      <ShieldBan size={14} />
                      禁止执行<span className="command-priority-tag">优先</span>
                    </span>
                    <small>
                      {parseCommandList(blockedDraft).length}
                      <em> / {MAX_COMMAND_RULES}</em>
                    </small>
                  </span>
                  <Textarea
                    aria-label="命令黑名单"
                    className="mono"
                    rows={10}
                    placeholder="添加需要始终拦截的命令"
                    value={blockedDraft}
                    disabled={saving}
                    onChange={(event) => setBlockedDraft(event.target.value)}
                  />
                </label>
              </div>
              {error && (
                <p className="error-inline" role="alert">
                  {error}
                </p>
              )}
            </div>
            <div className="modal-foot flex gap-2 justify-end">
              <Button
                type="button"
                variant="ghost"
                className="settings-editor-reset"
                disabled={saving}
                onClick={() => {
                  setAllowedDraft(defaultAllowedCommands.join("\n"));
                  setBlockedDraft("");
                  setError("");
                }}
              >
                <RotateCcw size={13} />
                恢复默认名单
              </Button>
              <Button
                type="button"
                variant="outline"
                disabled={saving}
                onClick={() => {
                  setListsOpen(false);
                  setError("");
                }}
              >
                取消
              </Button>
              <Button
                type="submit"
                className="command-save-button"
                disabled={saving}
              >
                <Check size={15} />
                {saving ? "保存中…" : "保存名单"}
              </Button>
            </div>
          </form>
        </ModalFrame>
      )}
      <footer className="settings-footer">
        <span role="status">
          {saving
            ? "保存中…"
            : error
              ? "保存失败"
              : desktop
                ? "设置自动保存"
                : "演示模式 · 业务设置仅保存在页面内存"}
        </span>
        <button className="text-btn" onClick={() => update(defaults)}>
          恢复界面默认设置
        </button>
      </footer>
    </div>
  );
}
