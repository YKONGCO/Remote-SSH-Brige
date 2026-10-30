import { api, desktop } from "../api";
import type {
  CommandPolicy,
  RuntimeSettings,
  NumericRuntimeSetting,
} from "../runtime-settings";
import { effectiveCommandPolicy } from "../runtime-settings";
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
  function commandToggle(name: keyof CommandPolicy, label: string) {
    const policy = runtime ? effectiveCommandPolicy(runtime) : undefined;
    return (
      <button
        type="button"
        className="setting-switch"
        role="switch"
        aria-label={label}
        aria-checked={policy?.[name] ?? false}
        disabled={!runtime || saving}
        onClick={() =>
          policy &&
          void updateRuntime("command_policy", {
            ...policy,
            [name]: !policy[name],
          })
        }
      >
        <span />
      </button>
    );
  }
  return (
    <div className="settings-layout" aria-busy={saving}>
      {error && (
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
            title="删除保护"
            description="阻止桌面和 MCP 删除远程文件与目录。"
          >
            {securityToggle("deletion_protection", "删除保护")}
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
            title="仅允许诊断命令"
            description="限制每个命令为诊断白名单；关闭后允许其他程序，高危命令拦截仍独立生效。"
          >
            {commandToggle("diagnostic_only", "仅允许诊断命令")}
          </Row>
          <Row
            title="允许管道"
            description="允许 ls | head 等管道；管道两侧命令分别检查。"
          >
            {commandToggle("allow_pipes", "允许管道")}
          </Row>
          <Row
            title="允许重定向"
            description="允许 >、>>、<、2>&1 等文件与文件描述符重定向。"
          >
            {commandToggle("allow_redirection", "允许重定向")}
          </Row>
          <Row
            title="允许组合命令"
            description="允许分号、换行、&&、|| 和后台执行；逐条检查命令。"
          >
            {commandToggle("allow_compound", "允许组合命令")}
          </Row>
          <Row
            title="允许脚本执行"
            description="允许解释器、脚本文件、变量与控制结构；需关闭诊断白名单。脚本文件及任意解释器代码无法完整静态检查。"
          >
            {commandToggle("allow_scripts", "允许脚本执行")}
          </Row>
          <Row
            title="允许命令替换"
            description="允许 $(...)、反引号和进程替换；嵌套的 Shell 命令同样检查。"
          >
            {commandToggle("allow_substitution", "允许命令替换")}
          </Row>
          <Row
            title="允许 MCP 交互终端"
            description="允许 MCP 打开终端和发送输入，仍需服务器授权；分段输入不经过高危命令检查。"
          >
            {commandToggle("allow_mcp_terminal", "允许 MCP 交互终端")}
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
