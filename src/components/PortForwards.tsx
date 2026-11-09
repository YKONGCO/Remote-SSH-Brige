import { useCallback, useEffect, useState, type FormEvent } from "react";
import { LoaderCircle, Plus, Power, Search, Trash2 } from "lucide-react";
import { api, desktop } from "../api";
import type { Server } from "../types";
import type { TunnelInfo } from "../tunnels";
import { usePreferences } from "../preferences";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { SelectField } from "./SelectField";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { CompactTime } from "./CompactTime";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";

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
  const [reopening, setReopening] = useState("");
  const [serverId, setServerId] = useState("");
  const [localPort, setLocalPort] = useState("0");
  const [remotePort, setRemotePort] = useState("8080");
  const [remoteHost, setRemoteHost] = useState("127.0.0.1");
  const [formError, setFormError] = useState("");
  const [filter, setFilter] = useState("all");
  const [status, setStatus] = useState("running");
  const [search, setSearch] = useState("");
  const [deleting, setDeleting] = useState<TunnelInfo | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const hasClosing = items.some((item) => item.status === "closing");
  const visible = items.filter(
    (item) =>
      (filter === "all" || item.server_id === filter) &&
      (status === "all" ||
        (status === "running"
          ? item.status !== "closed"
          : item.status === "closed")) &&
      `${item.server_name} ${item.tunnel_id} ${item.local_host}:${item.local_port} ${item.remote_host}:${item.remote_port}`
        .toLowerCase()
        .includes(search.toLowerCase()),
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
    // Finish tracking an acknowledged close even when periodic refresh is disabled.
    const refreshMs = hasClosing
      ? 500
      : Number(preferences.refreshSeconds) * 1000;
    const timer = refreshMs
      ? setInterval(() => void refresh(), refreshMs)
      : undefined;
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [refreshKey, preferences.refreshSeconds, hasClosing]);
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
      setStatus("running");
      setFilter("all");
      setSearch("");
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
  async function reopen(item: TunnelInfo) {
    if (reopening) return;
    setReopening(item.tunnel_id);
    try {
      await api("port_forward", {
        args: {
          action: "open",
          server_id: item.server_id,
          local_port: item.local_port,
          remote_host: item.remote_host,
          remote_port: item.remote_port,
        },
      });
      setStatus("running");
      setFilter("all");
      setSearch("");
      notify("端口转发已重新启用");
      await load();
    } catch (e) {
      notify(String(e), true);
    } finally {
      setReopening("");
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
          <Tabs
            className="audit-filters"
            value={status}
            onValueChange={setStatus}
          >
            <TabsList aria-label="端口转发状态">
              <TabsTrigger value="running">运行中</TabsTrigger>
              <TabsTrigger value="closed">已关闭</TabsTrigger>
              <TabsTrigger value="all">全部</TabsTrigger>
            </TabsList>
          </Tabs>
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
        <div className="audit-search search-box">
          <Search size={14} />
          <Input
            aria-label="搜索端口转发"
            placeholder="搜索端口或服务器"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
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
                <th className="tunnel-actions-column">操作</th>
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
                  <td className="tunnel-actions-column">
                    {item.status === "closed" ? (
                      <div className="tunnel-actions">
                        <Button
                          variant="outline"
                          size="sm"
                          className="tunnel-action-button tunnel-reopen-button"
                          aria-label={`启用端口转发 ${item.tunnel_id}`}
                          aria-busy={reopening === item.tunnel_id}
                          title={
                            servers.some(
                              (s) => s.id === item.server_id && s.enabled,
                            )
                              ? "使用原服务器和端口配置重新启用"
                              : "原服务器已删除或停用"
                          }
                          disabled={
                            !!reopening ||
                            !servers.some(
                              (s) => s.id === item.server_id && s.enabled,
                            )
                          }
                          onClick={() => void reopen(item)}
                        >
                          {reopening === item.tunnel_id ? (
                            <LoaderCircle size={14} className="spinning" />
                          ) : (
                            <Power size={14} />
                          )}
                          启用
                        </Button>
                        <Button
                          variant="outline"
                          size="sm"
                          className="tunnel-action-button tunnel-delete-button"
                          aria-label={`删除端口转发 ${item.tunnel_id}`}
                          disabled={!!reopening}
                          onClick={() => setDeleting(item)}
                        >
                          <Trash2 size={14} /> 删除
                        </Button>
                      </div>
                    ) : (
                      <Button
                        variant="outline"
                        size="sm"
                        className="tunnel-action-button"
                        disabled={item.status !== "running" || !!closing}
                        onClick={() => void close(item)}
                      >
                        <Power size={14} /> 关闭
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {!visible.length && (
            <div className="empty-panel">
              {status === "closed"
                ? "暂无已关闭的端口转发"
                : status === "running"
                  ? "暂无运行中的端口转发"
                  : "暂无端口转发"}
            </div>
          )}
        </div>
      </div>
      {deleting && (
        <ModalFrame
          className="confirm-modal"
          width={480}
          onClose={() => {
            if (!deleteBusy) setDeleting(null);
          }}
        >
          <DialogTitle>删除端口转发记录</DialogTitle>
          <p>
            删除 {deleting.server_name} 的 {deleting.local_port} →{" "}
            {deleting.remote_port} 转发记录？删除后无法恢复。
          </p>
          <div className="dialog-actions">
            <Button
              variant="outline"
              disabled={deleteBusy}
              onClick={() => setDeleting(null)}
            >
              取消
            </Button>
            <Button
              variant="destructive"
              disabled={deleteBusy}
              onClick={async () => {
                setDeleteBusy(true);
                try {
                  await api("delete_port_forward", { id: deleting.tunnel_id });
                  setDeleting(null);
                  notify("端口转发记录已删除");
                  await load();
                } catch (e) {
                  notify(String(e), true);
                } finally {
                  setDeleteBusy(false);
                }
              }}
            >
              {deleteBusy ? "正在删除…" : "删除记录"}
            </Button>
          </div>
        </ModalFrame>
      )}
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
            <p>
              远端地址指 SSH 服务器自身。历史记录会保留；重启应用后转发会关闭。
            </p>
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
