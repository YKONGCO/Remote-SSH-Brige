import { api, desktop } from "../api";
import type {
  CommandPolicy,
  RuntimeSettings,
  NumericRuntimeSetting,
} from "../runtime-settings";
import {
  effectiveCommandPolicy,
  commandMode,
  advancedShell,
  defaultAllowedCommands,
  validCommandLists,
} from "../runtime-settings";
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
  const [allowedDraft, setAllowedDraft] = useState("");
  const [blockedDraft, setBlockedDraft] = useState("");
  const policy = runtime ? effectiveCommandPolicy(runtime) : undefined;
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
  function securityToggle(
    name:
      "deletion_protection" | "block_dangerous_commands" | "remote_read_only",
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
      {error && !listsOpen && (
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
          <Row title="启动页面" description="下次打开应用时默认显示的页面。">
            {select("startPage", "启动页面", [
              ["servers", "服务器"],
              ["files", "远程文件"],
              ["terminals", "终端会话"],
              ["audit", "操作记录"],
              ["mcp", "MCP 服务"],
              ["settings", "设置"],
            ])}
          </Row>
          <Row
            title="关闭窗口时"
            description="选择退出应用，或保持后台运行并收起到托盘。"
          >
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
            title="状态刷新频率"
            description="更新服务器、会话与操作记录；不影响终端输出。"
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
          <Row
            title="主题外观"
            description="选择喜欢的色调，或跟随系统自动切换。"
          >
            {select("theme", "主题外观", [
              ["light", "浅色"],
              ["dark", "深色"],
              ["system", "跟随系统"],
            ])}
          </Row>
          <Row title="界面密度" description="调整服务器列表的行间距。">
            {select("density", "界面密度", [
              ["comfortable", "舒适"],
              ["compact", "紧凑"],
            ])}
          </Row>
          <Row title="界面动效" description="启用按钮与面板的轻量过渡。">
            {toggle("motion", "界面动效")}
          </Row>
        </div>
      </section>
      <section className="settings-section" id="security-settings">
        <h2 className="settings-group-title">安全控制</h2>
        <div className="settings-card">
          <Row
            title="桌面删除保护"
            description="阻止桌面文件管理中的删除；MCP 安全删除由服务器的独立授权控制。"
          >
            {securityToggle("deletion_protection", "桌面删除保护")}
          </Row>
          <Row
            title="危险命令拦截"
            description="拦截可解析 Shell 命令中的 rm、磁盘破坏、关机等已知高危操作。"
          >
            {securityToggle("block_dangerous_commands", "危险命令拦截")}
          </Row>
          <Row
            title="只读保护"
            description="禁止修改文件、执行命令和终端输入；查看与下载仍可使用。"
          >
            {securityToggle("remote_read_only", "只读保护")}
          </Row>
          <Row
            title="审批模式"
            description="修改文件、执行命令或打开终端前审批；请求 2 分钟后自动失效。"
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
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">命令与 Shell 权限</h2>
        <div className="settings-card">
          <Row
            title="命令执行范围"
            description="不限命令仍受黑名单和危险命令拦截限制。"
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
            description="添加、修改或删除白名单和黑名单，黑名单优先。"
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
            description="统一控制管道、组合命令、重定向、命令替换和脚本执行。脚本内容无法完整检查。"
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
            description="允许 AI 打开终端和发送输入，仍需服务器授权；终端输入不经过命令名单检查。"
          >
            {commandToggle("allow_mcp_terminal", "允许 AI 交互终端")}
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">连接与命令</h2>
        <div className="settings-card">
          <Row
            title="连接超时"
            description="每个服务器地址建立 TCP 连接的最长等待时间。"
          >
            {runtimeSelect(
              "connect_timeout_secs",
              "连接超时",
              [3, 8, 15, 30, 60, 120],
              "秒",
            )}
          </Row>
          <Row
            title="读写超时"
            description="SSH 握手、认证和文件传输的单次阻塞等待时间。"
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
            description="MCP 未指定执行时限时使用，可单次覆盖，最长 300 秒。"
          >
            {runtimeSelect(
              "command_timeout_secs",
              "默认命令超时",
              [10, 30, 60, 120, 300],
              "秒",
            )}
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">远程文件</h2>
        <div className="settings-card">
          <Row
            title="显示隐藏文件"
            description="显示以点开头的文件与文件夹，例如 .ssh。"
          >
            {toggle("showHidden", "显示隐藏文件")}
          </Row>
          <Row
            title="单文件上传上限"
            description="支持 1–256 MiB，离开输入框或按回车保存；同时限制文本写入。"
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
            description="支持 1–256 MiB，离开输入框或按回车保存；同时限制文本读取。"
          >
            <TransferLimitField
              label="单文件下载上限"
              value={runtime?.download_limit_mib}
              disabled={!runtime || saving}
              onSave={(value) => updateRuntime("download_limit_mib", value)}
            />
          </Row>
        </div>
      </section>
      <section className="settings-section">
        <h2 className="settings-group-title">终端</h2>
        <div className="settings-card">
          <Row title="终端配色" description="独立设置终端背景，默认使用深色。">
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
          <Row title="光标闪烁" description="让当前输入位置更容易识别。">
            {toggle("cursorBlink", "光标闪烁")}
          </Row>
          <Row
            title="回滚行数"
            description="终端视图保留的历史行数，不改变服务端输出缓冲上限。"
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
            description="限制新建会话数量；降低上限不会关闭已有会话。"
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
      {listsOpen && (
        <ModalFrame
          onClose={() => {
            if (!saving) {
              setListsOpen(false);
              setError("");
            }
          }}
          width={640}
        >
          <form
            className="modal-layout"
            onSubmit={async (event) => {
              event.preventDefault();
              const parse = (value: string) => [
                ...new Set(
                  value
                    .split(/\r?\n/)
                    .map((line) => line.trim())
                    .filter(Boolean),
                ),
              ];
              const allowed_commands = parse(allowedDraft),
                blocked_commands = parse(blockedDraft);
              if (!validCommandLists(allowed_commands, blocked_commands)) {
                setError(
                  "每个名单最多 256 项，仅填写命令名称或完整路径，不要填写参数、通配符或 Shell 语法。",
                );
                return;
              }
              if (await updateCommand({ allowed_commands, blocked_commands }))
                setListsOpen(false);
            }}
          >
            <div className="modal-head">
              <DialogTitle>命令名单管理</DialogTitle>
            </div>
            <div className="modal-body">
              <p>
                每行一条命令名称或完整路径，例如 grep 或
                /usr/local/bin/report。黑名单优先；仅白名单模式下，白名单留空会拒绝所有命令。
              </p>
              <div className="command-list-editors">
                <label>
                  白名单
                  <Textarea
                    aria-label="命令白名单"
                    className="mono"
                    rows={9}
                    value={allowedDraft}
                    disabled={saving}
                    onChange={(event) => setAllowedDraft(event.target.value)}
                  />
                </label>
                <label>
                  黑名单
                  <Textarea
                    aria-label="命令黑名单"
                    className="mono"
                    rows={9}
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
                disabled={saving}
                onClick={() => {
                  setAllowedDraft(defaultAllowedCommands.join("\n"));
                  setBlockedDraft("");
                  setError("");
                }}
              >
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
              <Button type="submit" disabled={saving}>
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
