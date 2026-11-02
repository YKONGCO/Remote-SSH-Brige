import { useCallback, useEffect, useState, type FormEvent } from "react";
import { Plus } from "lucide-react";
import { api, desktop } from "../api";
import type { Server } from "../types";
import type { TunnelInfo } from "../tunnels";
import { usePreferences } from "../preferences";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { SelectField } from "./SelectField";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { CompactTime } from "./CompactTime";

const states = { running: "运行中", closing: "关闭中", closed: "已关闭" };
function bytes(n: number) {
  return n < 1024
    ? `${n} B`
    : n < 1048576
      ? `${(n / 1024).toFixed(1)} KiB`
      : `${(n / 1048576).toFixed(1)} MiB`;
}
export function PortForwards({
  servers,
  refreshKey = 0,
  notify,
}: {
  servers: Server[];
  refreshKey?: number;
  notify: (message: string, error?: boolean) => void;
}) {
  const preferences = usePreferences();
  const [items, setItems] = useState<TunnelInfo[]>([]);
  const [error, setError] = useState("");
  const [creating, setCreating] = useState(false);
  const [busy, setBusy] = useState(false);
  const [closing, setClosing] = useState("");
  const [serverId, setServerId] = useState("");
  const [localPort, setLocalPort] = useState("0");
  const [remotePort, setRemotePort] = useState("8080");
  const [remoteHost, setRemoteHost] = useState("127.0.0.1");
  const [formError, setFormError] = useState("");
  const [filter, setFilter] = useState("all");
  const visible = items.filter(
    (item) => filter === "all" || item.server_id === filter,
  );
  const load = useCallback(async () => {
    try {
      const result = await api<{ tunnels: TunnelInfo[] }>("list_port_forwards");
      setItems(result.tunnels);
      setError("");
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    let disposed = false,
      inFlight = false;
    const refresh = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const result = await api<{ tunnels: TunnelInfo[] }>(
          "list_port_forwards",
        );
        if (!disposed) {
          setItems(result.tunnels);
          setError("");
        }
      } catch (e) {
        if (!disposed) setError(String(e));
      } finally {
        inFlight = false;
      }
    };
    void refresh();
    const timer =
      preferences.refreshSeconds === "0"
        ? undefined
        : setInterval(
            () => void refresh(),
            Number(preferences.refreshSeconds) * 1000,
          );
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [refreshKey, preferences.refreshSeconds]);
  async function open(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setFormError("");
    try {
      await api("port_forward", {
        args: {
          action: "open",
          server_id: serverId,
          local_port: Number(localPort),
          remote_host: remoteHost,
          remote_port: Number(remotePort),
        },
      });
      setCreating(false);
      notify("端口转发已创建");
      await load();
    } catch (e) {
      setFormError(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function close(item: TunnelInfo) {
    setClosing(item.tunnel_id);
    try {
      await api("port_forward", {
        args: {
          action: "close",
          server_id: item.server_id,
          tunnel_id: item.tunnel_id,
        },
      });
      notify("已请求关闭端口转发");
      await load();
    } catch (e) {
      notify(String(e), true);
    } finally {
      setClosing("");
    }
  }
  return (
    <section className="tunnels-page">
      {!desktop && (
        <p className="tunnel-demo-note">
          浏览器演示：转发仅模拟显示，真实端口转发请使用桌面应用。
        </p>
      )}
      <div className="audit-toolbar tunnels-toolbar">
        <div className="audit-filter-summary">
          <SelectField
            aria-label="筛选转发服务器"
            className="tunnels-server-filter"
            value={filter}
            onValueChange={setFilter}
          >
            <option value="all">全部服务器</option>
            {servers.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
              </option>
            ))}
          </SelectField>
          <span
            className="audit-record-count"
            role="status"
            aria-label="端口转发数量"
          >
            {visible.length} 个
          </span>
        </div>
        <Button
          size="sm"
          className="btn primary"
          disabled={!servers.some((s) => s.enabled)}
          onClick={() => {
            setServerId(servers.find((s) => s.enabled)?.id || "");
            setFormError("");
            setCreating(true);
          }}
        >
          <Plus size={14} />
          新建转发
        </Button>
      </div>
      {error && <p role="alert">{error}</p>}
      <div className="panel">
        <div
          className="table-scroll tunnels-table-scroll"
          aria-label="端口转发列表"
        >
          <table className="data-table audit-table tunnels-table">
            <thead>
              <tr>
                <th>端口映射</th>
                <th>服务器 / 来源</th>
                <th>状态</th>
                <th>连接 / 流量</th>
                <th>创建时间</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((item) => (
                <tr key={item.tunnel_id}>
                  <td>
                    <strong className="mono">
                      127.0.0.1:{item.local_port} →{" "}
                      {item.remote_host === "::1" ? "[::1]" : item.remote_host}:
                      {item.remote_port}
                    </strong>
                    <p className="mono">{item.tunnel_id}</p>
                  </td>
                  <td>
                    {item.server_name}
                    <p>{item.source === "mcp" ? "MCP" : "桌面"}</p>
                  </td>
                  <td>
                    <span
                      className={`job-status ${item.status === "running" ? "green" : "gray"}`}
                    >
                      {states[item.status]}
                    </span>
                    {item.error && (
                      <p className="tunnel-error" title={item.error}>
                        {item.error}
                      </p>
                    )}
                  </td>
                  <td>
                    {item.connections} 个连接
                    <p>
                      发送 {bytes(item.bytes_sent)} / 接收{" "}
                      {bytes(item.bytes_received)}
                    </p>
                  </td>
                  <td>
                    <CompactTime value={item.created_at} />
                  </td>
                  <td>
                    <Button
                      variant="outline"
                      disabled={item.status !== "running" || !!closing}
                      onClick={() => void close(item)}
                    >
                      关闭
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {!visible.length && <div className="empty-panel">暂无端口转发</div>}
        </div>
      </div>
      {creating && (
        <ModalFrame
          className="server-modal"
          onClose={() => {
            if (!busy) setCreating(false);
          }}
        >
          <form onSubmit={open} className="tunnel-form">
            <DialogTitle>新建端口转发</DialogTitle>
            <label>
              服务器
              <SelectField
                aria-label="转发服务器"
                value={serverId}
                onValueChange={setServerId}
              >
                {servers
                  .filter((s) => s.enabled)
                  .map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name} · {s.host}
                    </option>
                  ))}
              </SelectField>
            </label>
            <label>
              本地端口
              <Input
                aria-label="本地端口"
                type="number"
                min={0}
                max={65535}
                required
                value={localPort}
                onChange={(e) => setLocalPort(e.target.value)}
              />
            </label>
            <p>本地监听地址为 127.0.0.1；端口填 0 自动分配。</p>
            <label>
              远端地址
              <SelectField
                aria-label="远端地址"
                value={remoteHost}
                onValueChange={setRemoteHost}
              >
                <option value="127.0.0.1">127.0.0.1</option>
                <option value="::1">::1（IPv6）</option>
              </SelectField>
            </label>
            <label>
              远端端口
              <Input
                aria-label="远端端口"
                type="number"
                min={1}
                max={65535}
                required
                value={remotePort}
                onChange={(e) => setRemotePort(e.target.value)}
              />
            </label>
            <p>远端地址指 SSH 服务器自身。重启应用后转发不会恢复。</p>
            {formError && <p role="alert">{formError}</p>}
            <div className="dialog-actions">
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => setCreating(false)}
              >
                取消
              </Button>
              <Button type="submit" disabled={busy || !serverId}>
                {busy ? "正在创建…" : "创建转发"}
              </Button>
            </div>
          </form>
        </ModalFrame>
      )}
    </section>
  );
}
