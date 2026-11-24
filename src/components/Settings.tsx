import { api, desktop } from "../api";
import type {
  CommandPolicy,
  RuntimeSettings,
  NumericRuntimeSetting,
} from "../runtime-settings";
import {
  defaultTransferSettings,
  defaultAdvancedMcpTools,
  effectiveCommandPolicy,
  advancedShell,
  validProtectedCommandRules,
  parseProtectedCommandRules,
  MAX_COMMAND_RULES,
  RUNTIME_SETTINGS_CHANGED,
  type BuiltinRule,
} from "../runtime-settings";
import {
  X,
  RotateCcw,
  Check,
  ShieldBan,
  FolderOpen,
  ShieldAlert,
  ChevronRight,
} from "lucide-react";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";
import { Textarea } from "./ui/textarea";
import { SelectField } from "./SelectField";
import { TransferLimitField } from "./TransferLimitField";
import { useEffect, useRef, useState, type ReactNode } from "react";
import appPackage from "../../package.json";
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
export function Settings({
  initialRuntime,
}: {
  initialRuntime?: RuntimeSettings | null;
} = {}) {
  const preferences = usePreferences();
  const [error, setError] = useState("");
  const [runtime, setRuntime] = useState<RuntimeSettings | null>(
    initialRuntime ?? null,
  );
  const [saving, setSaving] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const loadRequest = useRef(0);
  const [listsOpen, setListsOpen] = useState(false);
  const [rootsEditor, setRootsEditor] = useState<
    "local_upload_roots" | "local_download_roots" | null
  >(null);
  const [rootsDraft, setRootsDraft] = useState("");
  const [pickingDirectories, setPickingDirectories] = useState(false);
  const [protectedDraft, setProtectedDraft] = useState("");
  const [builtinRules, setBuiltinRules] = useState<BuiltinRule[]>([]);
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
    return updateRuntime("command_policy", { ...policy, ...patch });
  }
  async function openRules() {
    if (!policy || !runtime) return;
    setProtectedDraft(policy.protected_commands.join("\n"));
    setListsOpen(true);
    setError("");
    try {
      const rules = await api<{ builtin: BuiltinRule[] }>("protected_rules");
      setBuiltinRules(rules.builtin);
    } catch (e) {
      setError(String(e));
    }
  }
  function setAdvancedShell(enabled: boolean) {
    void updateCommand({
      allow_pipes: enabled,
      allow_redirection: enabled,
      allow_compound: enabled,
      allow_scripts: enabled,
      allow_substitution: enabled,
    });
  }
  async function loadRuntime() {
    const request = ++loadRequest.current;
    try {
      const settings = await api<RuntimeSettings>("runtime_settings");
      if (request !== loadRequest.current) return;
      setRuntime((current) =>
        current && JSON.stringify(current) === JSON.stringify(settings)
          ? current
          : settings,
      );
      setLoadFailed(false);
      setError("");
    } catch (e) {
      if (request !== loadRequest.current) return;
      setError(String(e));
      setLoadFailed(true);
    }
  }
  useEffect(() => {
    void loadRuntime();
    return () => {
      ++loadRequest.current;
    };
  }, []);
  async function updateRuntime<K extends keyof RuntimeSettings>(
    name: K,
    value: RuntimeSettings[K],
  ) {
    if (!runtime || saving) return false;
    ++loadRequest.current;
    const previous = runtime;
    const next = {
      ...runtime,
      command_policy: effectiveCommandPolicy(runtime),
      [name]: value,
    };
    setRuntime(next);
    setSaving(true);
    try {
      const saved = await api<RuntimeSettings>("save_runtime_settings", {
        settings: next,
      });
      setRuntime(saved);
      setLoadFailed(false);
      window.dispatchEvent(new Event(RUNTIME_SETTINGS_CHANGED));
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
  function toggle(name: "showHidden" | "cursorBlink", label: string) {
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
      | "remote_read_only"
      | "mcp_auto_start"
      | "approval_mode"
      | "exec_timeout_to_job",
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
    name: "allow_advanced_shell" | "exec_disabled",
    label: string,
  ) {
    const checked = policy
      ? name === "allow_advanced_shell"
        ? advancedShell(policy)
        : policy[name]
      : false;
    return (
      <button
        type="button"
        className="setting-switch"
        role="switch"
        aria-label={label}
        aria-checked={checked}
        disabled={!runtime || saving}
        onClick={() => {
          if (!policy) return;
          if (name === "allow_advanced_shell") setAdvancedShell(!checked);
          else void updateCommand({ [name]: !checked });
        }}
      >
        <span />
      </button>
    );
  }
  function advancedToggle(name: "terminal" | "port_forward", label: string) {
    const tools = {
      ...defaultAdvancedMcpTools,
      ...runtime?.advanced_mcp_tools,
    };
    return (
      <button
        type="button"
        className="setting-switch"
        role="switch"
        aria-label={label}
        aria-checked={tools[name]}
        disabled={!runtime || saving}
        onClick={() =>
          void updateRuntime("advanced_mcp_tools", {
            ...tools,
            [name]: !tools[name],
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
          <Row title="主题外观" description="浅色、深色或跟随系统。">
            {select("theme", "主题外观", [
              ["light", "浅色"],
              ["dark", "深色"],
              ["system", "跟随系统"],
            ])}
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">MCP</h2>
        <div className="settings-card">
          <Row
            title="自动启动 MCP 服务"
            description="打开应用时使用上次成功启动的地址和端口。"
          >
            {runtimeToggle("mcp_auto_start", "自动启动 MCP 服务")}
          </Row>
          <Row
            title="交互式终端"
            description="允许 Agent 在已授权的服务器上使用交互式 Shell，不经过 Exec 命令保护。"
          >
            {advancedToggle("terminal", "交互式终端")}
          </Row>
          <Row
            title="端口转发"
            description="允许 Agent 在已授权的服务器上创建和管理 SSH 端口转发。"
          >
            {advancedToggle("port_forward", "端口转发")}
          </Row>
          <Row
            title="默认命令超时"
            description="MCP 命令的默认时限，可按次调整。"
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
            {runtimeToggle("exec_timeout_to_job", "命令超时后保留为后台任务")}
          </Row>
        </div>
      </section>
      <section className="settings-section" id="security-settings">
        <h2 className="settings-group-title">安全</h2>
        <div className="settings-card">
          <Row
            title="只读保护"
            description="禁止远端文件修改及命令执行，仅允许受支持的读取操作。"
          >
            {runtimeToggle("remote_read_only", "只读保护")}
          </Row>
          <Row
            title="删除保护"
            description="禁止通过文件管理功能删除远端文件，不限制 Shell 命令。"
          >
            {runtimeToggle("deletion_protection", "删除保护")}
          </Row>
          <Row
            title="允许高危命令审批"
            description="开启后，Agent 可通过 exec_approved 申请执行受保护命令。关闭后直接拒绝。"
          >
            {runtimeToggle("approval_mode", "允许高危命令审批")}
          </Row>
          <Row
            title="受保护命令"
            description="内置规则始终生效，可额外添加需要审批的命令。"
          >
            <Button
              variant="outline"
              size="sm"
              disabled={!runtime || saving}
              onClick={() => void openRules()}
            >
              <ShieldAlert size={14} />
              管理受保护命令
            </Button>
          </Row>
          {policy?.legacy_whitelist ? (
            <Row
              title="旧版白名单限制"
              description={`已保留 ${policy.legacy_whitelist.length} 条旧规则。切换后，名单外的命令按新版规则判断。`}
            >
              <Button
                variant="outline"
                size="sm"
                disabled={saving}
                onClick={() =>
                  void updateCommand({
                    legacy_whitelist: null,
                  })
                }
              >
                改用新版受保护命令策略
              </Button>
            </Row>
          ) : null}
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">文件传输</h2>
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
          <Row title="自动重试次数" description="可重试的传输失败会自动重试。">
            {transferSelect(
              "max_retries",
              "自动重试次数",
              [0, 1, 3, 5, 10],
              "次",
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
          <Row title="回滚行数" description="终端界面保留的历史行数。">
            {select(
              "terminalScrollback",
              "回滚行数",
              [1000, 5000, 10000, 20000].map((n) => [
                String(n),
                `${n.toLocaleString()} 行`,
              ]),
            )}
          </Row>
        </div>
      </section>
      <details className="settings-section settings-advanced">
        <summary className="settings-group-title">
          <ChevronRight size={16} aria-hidden="true" />
          高级设置
        </summary>
        <div className="settings-card">
          <Row
            title="禁止命令执行"
            description="禁用所有服务器的 MCP 命令执行，包括审批执行。"
          >
            {commandToggle("exec_disabled", "禁止命令执行")}
          </Row>
          <Row
            title="允许 Shell 高级功能"
            description="允许管道、重定向、组合命令及脚本。命中受保护规则时仍需审批。"
          >
            {commandToggle("allow_advanced_shell", "允许 Shell 高级功能")}
          </Row>
          <Row
            title="审批等待时间"
            description="超时后自动拒绝。修改仅影响新请求。"
          >
            {runtimeSelect(
              "approval_timeout_secs",
              "审批等待时间",
              [1, 5, 10, 15, 30, 60, 120],
              "秒",
            )}
          </Row>
          <Row title="连接超时" description="建立服务器连接的最长等待时间。">
            {runtimeSelect(
              "connect_timeout_secs",
              "连接超时",
              [3, 8, 15, 30, 60, 120],
              "秒",
            )}
          </Row>
          <Row title="握手超时" description="等待 SSH 握手完成的最长时间。">
            {runtimeSelect(
              "handshake_timeout_secs",
              "握手超时",
              [5, 15, 30, 60, 120],
              "秒",
            )}
          </Row>
          <Row title="读写超时" description="普通 SSH 操作的单次等待上限。">
            {runtimeSelect(
              "io_timeout_secs",
              "读写超时",
              [5, 15, 30, 60, 120, 300],
              "秒",
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
            description="传输无进展时的最长等待时间。"
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
            description="单个任务的默认速度上限，0 表示不限。"
          >
            {transferSelect(
              "bandwidth_kib",
              "默认传输限速",
              [0, 128, 512, 1024, 4096, 16384],
              "KiB/s",
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
      </details>
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
          width={720}
        >
          <form
            className="modal-layout"
            onSubmit={async (event) => {
              event.preventDefault();
              const protected_commands =
                parseProtectedCommandRules(protectedDraft);
              if (!validProtectedCommandRules(protected_commands)) {
                setError(
                  `最多 ${MAX_COMMAND_RULES} 条，每行填写命令名或绝对路径，可带固定参数。不支持通配符和 Shell 语法。`,
                );
                return;
              }
              if (await updateCommand({ protected_commands })) {
                setListsOpen(false);
                setError("");
              }
            }}
          >
            <div className="modal-head settings-editor-head">
              <DialogTitle>受保护命令</DialogTitle>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="command-modal-close"
                aria-label="关闭受保护命令管理"
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
              <div className="command-builtin">
                <span className="command-editor-label">
                  <span>
                    <ShieldAlert size={14} />
                    内置规则（只读，始终启用）
                  </span>
                  <small>{builtinRules.length} 条</small>
                </span>
                <ul className="protected-rule-list">
                  {builtinRules.map((rule) => (
                    <li key={rule.command}>
                      <code>
                        {rule.command}
                        {rule.arguments?.length
                          ? ` ${rule.arguments.join(" / ")}`
                          : ""}
                      </code>
                      <span>{rule.reason}</span>
                    </li>
                  ))}
                  {!builtinRules.length && <li>正在加载内置规则…</li>}
                </ul>
              </div>
              <div
                className="command-list-editors"
                title="拖动右下角可调整输入区高度"
              >
                <label>
                  <span className="command-editor-label">
                    <span>
                      <ShieldBan size={14} />
                      自定义规则
                    </span>
                    <small>
                      {parseProtectedCommandRules(protectedDraft).length}
                      <em> / {MAX_COMMAND_RULES}</em>
                    </small>
                  </span>
                  <Textarea
                    aria-label="自定义受保护命令"
                    className="mono"
                    rows={8}
                    placeholder={
                      "每行一条，按命令开头匹配\ndocker rmi\ndocker compose down\n/opt/tools/deploy"
                    }
                    value={protectedDraft}
                    disabled={saving}
                    onChange={(event) => setProtectedDraft(event.target.value)}
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
                  setProtectedDraft("");
                  setError("");
                }}
              >
                <RotateCcw size={13} />
                清空自定义规则
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
                {saving ? "保存中…" : "保存规则"}
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
        <div className="settings-footer-actions">
          <span
            className="version-pill"
            aria-label={`应用版本 ${appPackage.version}`}
          >
            v{appPackage.version}
          </span>
          <button className="text-btn" onClick={() => update(defaults)}>
            恢复界面默认设置
          </button>
        </div>
      </footer>
    </div>
  );
}
