import { useEffect, useState } from "react";
import {
  AppWindow,
  Check,
  ChevronRight,
  Copy,
  FileCode2,
  Globe2,
  Plug,
  Play,
  Pi,
  RefreshCw,
  Server as ServerIcon,
  ShieldCheck,
  Square,
  WandSparkles,
} from "lucide-react";
import { api, desktop } from "../api";
import { mcpServerName } from "../branding";
import { clientNames, type ClientId, type McpClient } from "../mcp-clients";
import type { McpStatus, Server } from "../types";
import { mcpPermissionSummary } from "../mcp-permissions";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import claudeIcon from "../assets/clients/claude-color.svg";
import codexIcon from "../assets/clients/openai.svg";
import geminiIcon from "../assets/clients/gemini-color.svg";
import opencodeIcon from "../assets/clients/opencode.svg";
import cursorIcon from "../assets/clients/cursor.svg";
const icons = {
  claude: claudeIcon,
  codex: codexIcon,
  gemini: geminiIcon,
  opencode: opencodeIcon,
  cursor: cursorIcon,
  pi: null,
};
function mcpUrl(host: string, port: number) {
  return `http://${host.includes(":") ? `[${host}]` : host}:${port}/mcp`;
}
function ManualMcpConfig({
  status,
  notify,
}: {
  status: McpStatus;
  notify: (message: string, error?: boolean) => void;
}) {
  const [copied, setCopied] = useState(false);
  const [remoteAddress, setRemoteAddress] = useState("");
  const listenHost = status.host || "127.0.0.1";
  const remote = !["127.0.0.1", "::1"].includes(listenHost);
  const wildcard = ["0.0.0.0", "::"].includes(listenHost);
  const targetHost = remoteAddress.trim() || (wildcard ? "" : listenHost);
  const validHost = Boolean(targetHost) && /^[a-zA-Z0-9.:-]+$/.test(targetHost);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1800);
    return () => clearTimeout(timer);
  }, [copied]);
  const configuration = JSON.stringify(
    {
      mcpServers: {
        [mcpServerName]: {
          url: mcpUrl(remote ? targetHost : listenHost, status.port),
          headers: { Authorization: `Bearer ${status.token}` },
        },
      },
    },
    null,
    2,
  );
  return (
    <section className="mcp-manual-card">
      <div className="mcp-manual-toolbar">
        <h2>
          <FileCode2 size={16} />
          客户端配置
        </h2>
        <Button
          variant="outline"
          className="btn small"
          disabled={!status.running || !validHost}
          onClick={() =>
            void navigator.clipboard
              .writeText(configuration)
              .then(() => setCopied(true))
              .catch((e) => notify(String(e), true))
          }
        >
          {copied ? <Check size={14} /> : <Copy size={14} />}
          {copied ? "已复制" : "复制配置"}
        </Button>
      </div>
      {remote && (
        <label className="mcp-remote-address">
          客户端连接 IP
          <Input
            aria-label="客户端连接 IP"
            placeholder={
              wildcard ? "填写本机的局域网 IP，例如 192.168.1.10" : listenHost
            }
            value={remoteAddress}
            onChange={(event) => {
              setRemoteAddress(event.target.value);
              setCopied(false);
            }}
          />
        </label>
      )}
      <pre className="mcp-config-code">
        {status.running
          ? validHost
            ? configuration
            : "填写其他主机可访问的本机 IP 后生成配置。"
          : "启动服务后生成接入配置。"}
      </pre>
    </section>
  );
}
export function McpPanel({
  status,
  servers,
  port,
  setPort,
  host = "127.0.0.1",
  setHost,
  busy,
  toggle,
  edit,
  notify,
}: {
  status: McpStatus;
  servers: Server[];
  port: number;
  setPort: (port: number) => void;
  host?: string;
  setHost?: (host: string) => void;
  busy: boolean;
  toggle: () => Promise<void>;
  edit: (server: Server) => void;
  notify: (message: string, error?: boolean) => void;
}) {
  const [clients, setClients] = useState<McpClient[]>([]);
  const [loading, setLoading] = useState(true);
  const [configuring, setConfiguring] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    setLoading(true);
    void api<McpClient[]>("mcp_clients")
      .then((value) => {
        if (active) {
          setClients(value);
          setError("");
        }
      })
      .catch((e) => {
        if (active) setError(String(e));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [status.running, status.port, status.token, status.host]);
  async function configure(id: ClientId) {
    setConfiguring(id);
    setError("");
    try {
      setClients(await api<McpClient[]>("configure_mcp_client", { id }));
      notify(
        `${clientNames[id]} 已配置，请在客户端重新连接 ${mcpServerName}。`,
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setConfiguring("");
    }
  }
  return (
    <div className="mcp-workspace">
      <section className="mcp-service-card">
        <div className="mcp-service-summary">
          <span className="mcp-section-icon">
            <Plug size={20} />
          </span>
          <div>
            <strong>MCP 服务</strong>
            <p>通过 MCP 连接远程服务器、文件与终端</p>
          </div>
          <span className="mcp-state">
            <i
              className={"status-dot " + (status.running ? "green" : "gray")}
            />
            {status.running ? "运行中" : "已停止"}
          </span>
          <Button
            className={"btn " + (status.running ? "" : "primary")}
            disabled={busy}
            onClick={() => void toggle()}
          >
            {busy ? (
              <RefreshCw size={14} className="spinning" />
            ) : status.running ? (
              <Square size={13} />
            ) : (
              <Play size={13} />
            )}
            {busy ? "处理中…" : status.running ? "停止服务" : "启动服务"}
          </Button>
        </div>
        <div className="mcp-service-options">
          <label>
            <Globe2 size={15} />
            监听 IP
            <Input
              className="mcp-host-input"
              aria-label="MCP 监听 IP"
              value={status.running ? status.host || "127.0.0.1" : host}
              disabled={status.running || busy}
              onChange={(event) => setHost?.(event.target.value)}
            />
          </label>
          <label>
            端口
            <Input
              aria-label="MCP 服务端口"
              type="number"
              min={1}
              max={65535}
              disabled={status.running || busy}
              value={status.running ? status.port : port}
              onChange={(e) => setPort(Number(e.target.value))}
            />
          </label>
          <span>
            <ShieldCheck size={15} />
            {
              servers.filter((s) => s.enabled && s.mcp_enabled !== false).length
            }{" "}
            台服务器可用
          </span>
          <span className="mcp-transport">Streamable HTTP</span>
        </div>
        {status.error && (
          <p role="alert" className="error-inline">
            {status.error}
          </p>
        )}
      </section>
      <section className="mcp-clients-section">
        <div className="mcp-section-title">
          <h2>
            <AppWindow size={16} />
            客户端接入
          </h2>
          <span>用户级配置 · 自动备份</span>
        </div>
        <div className="mcp-client-list">
          {loading && clients.length === 0 ? (
            <div className="mcp-loading" role="status">
              正在检测客户端配置…
            </div>
          ) : (
            clients.map((client) => (
              <div className="mcp-client-row" key={client.id}>
                <span className={"mcp-brand " + client.id}>
                  {client.id === "pi" ? (
                    <Pi size={22} />
                  ) : (
                    <img src={icons[client.id]!} alt="" />
                  )}
                </span>
                <div className="mcp-client-info">
                  <strong>{client.name}</strong>
                  <span className="mcp-client-path" title={client.path}>
                    {client.path}
                  </span>
                  {client.error && (
                    <span className="mcp-client-error">{client.error}</span>
                  )}
                </div>
                <span className={"mcp-client-state " + client.status}>
                  {!desktop
                    ? "仅桌面可配置"
                    : client.status === "configured"
                      ? "已配置"
                      : client.status === "outdated"
                        ? "需要更新"
                        : client.status === "error"
                          ? "配置异常"
                          : client.status === "missing"
                            ? "将创建配置"
                            : "未配置"}
                </span>
                <Button
                  variant="outline"
                  className="btn small"
                  disabled={
                    !desktop ||
                    !status.running ||
                    !!configuring ||
                    busy ||
                    client.status === "error" ||
                    client.status === "configured"
                  }
                  onClick={() => void configure(client.id)}
                >
                  {configuring === client.id ? (
                    <RefreshCw size={13} className="spinning" />
                  ) : client.status === "configured" ? (
                    <Check size={13} />
                  ) : client.status === "outdated" ? (
                    <RefreshCw size={13} />
                  ) : (
                    <WandSparkles size={13} />
                  )}
                  {configuring === client.id
                    ? "配置中…"
                    : client.status === "configured"
                      ? "已配置"
                      : client.status === "outdated"
                        ? "更新配置"
                        : "一键配置"}
                </Button>
              </div>
            ))
          )}
        </div>
        {error && (
          <p role="alert" className="error-inline">
            {error}
            <Button
              variant="ghost"
              className="text-btn"
              onClick={() => {
                setLoading(true);
                void api<McpClient[]>("mcp_clients")
                  .then(setClients)
                  .then(() => setError(""))
                  .catch((e) => setError(String(e)))
                  .finally(() => setLoading(false));
              }}
            >
              重新检测
            </Button>
          </p>
        )}
        <p className="mcp-note">
          {status.running
            ? "服务重启后令牌会更新，显示“需要更新”时请重新配置。"
            : "启动服务后可一键配置客户端，也可复制对应格式手动接入。"}
        </p>
      </section>
      <ManualMcpConfig status={status} notify={notify} />
      <section className="mcp-permissions">
        <div className="mcp-section-title">
          <h2>
            <ShieldCheck size={16} />
            服务器权限
          </h2>
          <span>选择服务器修改授权</span>
        </div>
        <div className="mcp-permission-list">
          {servers.map((server) => (
            <button
              className="mcp-permission-row"
              key={server.id}
              onClick={() => edit(server)}
            >
              <ServerIcon size={17} />
              <span>{server.name}</span>
              <span className="mcp-permission-values">
                {mcpPermissionSummary(server)}
              </span>
              <ChevronRight size={14} />
            </button>
          ))}
          {!servers.length && <div className="empty-small">请先添加服务器</div>}
        </div>
        <p className="mcp-note">
          命令和终端使用登录账号的完整权限，不受文件根目录限制。其他主机请使用上方客户端配置，本机一键配置会自动匹配监听地址。
        </p>
      </section>
    </div>
  );
}
