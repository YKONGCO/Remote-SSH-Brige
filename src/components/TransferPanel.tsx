import { useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  Download,
  FileDown,
  FileUp,
  FolderOpen,
  LoaderCircle,
  Search,
  Trash2,
  XCircle,
  Pause,
  Play,
  RotateCcw,
} from "lucide-react";
import { api, desktop } from "../api";
import { formatBytes, type FileTransfer } from "../transfers";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";

const terminal = (s: string) =>
  ["completed", "failed", "cancelled"].includes(s);
const phases: Record<string, string> = {
  preparing: "准备中",
  awaiting_approval: "等待审批",
  transferring: "传输中",
  committing: "正在提交文件",
  cancelling: "正在取消",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
  queued: "排队中",
  connecting: "正在连接",
  retrying: "等待自动重试",
  paused: "已暂停",
  pausing: "正在暂停",
  hashing: "正在计算校验值",
  verifying: "正在校验文件",
  reconciling: "正在确认提交结果",
  skipped: "内容相同，已跳过",
};

export function TransferPanel({
  serverId,
  onComplete,
  notify,
}: {
  serverId: string;
  onComplete: () => void;
  notify: (message: string, error?: boolean) => void;
}) {
  const [items, setItems] = useState<FileTransfer[]>([]);
  const [error, setError] = useState("");
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState<Set<string>>(new Set());
  const callbacks = useRef({ onComplete, notify });
  callbacks.current = { onComplete, notify };
  useEffect(() => {
    if (!desktop) return;
    let disposed = false,
      loading = false;
    const known = new Map<string, string>();
    setItems([]);
    setDismissed(new Set());
    setSearch("");
    setSearchOpen(false);
    const poll = async () => {
      if (loading) return;
      loading = true;
      try {
        const result = await api<{ transfers: FileTransfer[] }>("transfer", {
          serverId,
          action: "list",
        });
        if (disposed) return;
        setError("");
        setItems(result.transfers);
        for (const item of result.transfers) {
          const previous = known.get(item.transfer_id);
          if (previous && !terminal(previous) && terminal(item.status)) {
            callbacks.current.notify(
              item.status === "completed"
                ? "文件传输已完成"
                : item.error?.message || "文件传输已取消",
              item.status === "failed",
            );
            if (item.status === "completed") callbacks.current.onComplete();
          }
          known.set(item.transfer_id, item.status);
        }
      } catch (e) {
        if (!disposed) setError(String(e));
      } finally {
        loading = false;
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), 750);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [serverId]);
  if (!desktop) return null;
  const visibleItems = items.filter(
    (item) =>
      !dismissed.has(item.transfer_id) &&
      (!search ||
        item.remote_path
          .toLocaleLowerCase()
          .includes(search.toLocaleLowerCase())),
  );
  const activeCount = visibleItems.filter(
    (item) => !terminal(item.status) && item.status !== "paused",
  ).length;
  const groups = new Map<string, FileTransfer[]>();
  for (const item of visibleItems)
    if (item.group_id)
      groups.set(item.group_id, [...(groups.get(item.group_id) || []), item]);
  const latestCompleted = items.find(
    (item) => item.status === "completed" && !dismissed.has(item.transfer_id),
  );
  const reveal = (item: FileTransfer) =>
    api("reveal_transfer", {
      serverId,
      transferId: item.transfer_id,
    }).catch((e) => callbacks.current.notify(String(e), true));
  const control = async (item: FileTransfer, action: string, patch = {}) => {
    setBusy((current) => new Set(current).add(item.transfer_id));
    try {
      const result = await api<FileTransfer>("transfer", {
        serverId,
        action,
        transferId: item.transfer_id,
        ...patch,
      });
      if (action === "cleanup")
        setItems((current) =>
          current.filter((x) => x.transfer_id !== item.transfer_id),
        );
      else
        setItems((current) =>
          current.map((x) =>
            x.transfer_id === result.transfer_id ? result : x,
          ),
        );
    } catch (e) {
      callbacks.current.notify(String(e), true);
    } finally {
      setBusy((current) => {
        const next = new Set(current);
        next.delete(item.transfer_id);
        return next;
      });
    }
  };
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          className="btn transfer-menu-trigger"
          aria-label="传输任务"
          title="传输任务"
        >
          <Download size={17} />
          {activeCount > 0 && (
            <span className="transfer-menu-badge" aria-hidden="true" />
          )}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        className="transfer-menu"
        role="dialog"
        aria-label="文件传输任务"
      >
        <div className="transfer-menu-header">
          <strong>
            文件传输{activeCount ? ` · ${activeCount} 项进行中` : ""}
          </strong>
          <div className="transfer-menu-header-actions">
            <Button
              variant="ghost"
              size="icon"
              className="icon-btn"
              aria-label="打开下载文件夹"
              title="打开下载文件夹"
              disabled={!latestCompleted}
              onClick={() => latestCompleted && void reveal(latestCompleted)}
            >
              <FolderOpen size={17} />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="icon-btn"
              aria-label="搜索下载任务"
              title="搜索下载任务"
              aria-pressed={searchOpen}
              onClick={() => setSearchOpen((value) => !value)}
            >
              <Search size={17} />
            </Button>
          </div>
        </div>
        {searchOpen && (
          <div className="transfer-menu-search">
            <Search size={14} />
            <Input
              autoFocus
              aria-label="搜索下载任务"
              placeholder="搜索下载文件"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
          </div>
        )}
        {error && (
          <p className="error-inline" role="alert">
            {error}
          </p>
        )}
        {!visibleItems.length && !error && (
          <div className="transfer-menu-empty">
            {search ? "没有匹配的传输任务" : "暂无传输任务"}
          </div>
        )}
        {[...groups].slice(0, 5).map(([id, tasks]) => {
          const bytes = tasks.reduce((sum, t) => sum + t.transferred_bytes, 0);
          const total = tasks.reduce((sum, t) => sum + (t.total_bytes || 0), 0);
          return (
            <div className="transfer-group-progress" key={id}>
              <small title={id}>
                批次 {id.slice(0, 8)} · 已完成{" "}
                {tasks.filter((t) => t.status === "completed").length}/
                {tasks.length} · {formatBytes(bytes)}
                {total ? ` / ${formatBytes(total)}` : ""}
              </small>
              <progress
                aria-label={`批次 ${id} 进度`}
                max={total || 1}
                value={bytes}
              />
            </div>
          );
        })}
        {[
          ...visibleItems.filter((item) => !terminal(item.status)),
          ...visibleItems.filter((item) => terminal(item.status)),
        ]
          .slice(0, 10)
          .map((item) => (
            <div className="file-transfer-item" key={item.transfer_id}>
              <div className="file-transfer-card-row">
                <div className="file-transfer-title">
                  {item.direction === "upload" ? (
                    <FileUp className="transfer-file-icon" size={22} />
                  ) : (
                    <FileDown className="transfer-file-icon" size={22} />
                  )}
                  <div>
                    <span title={item.remote_path}>
                      {item.remote_path.split("/").pop() || item.remote_path}
                    </span>
                    <small>
                      {item.direction === "upload" ? "上传" : "下载"} ·{" "}
                      {phases[item.phase] || item.phase}
                    </small>
                  </div>
                </div>
                <div className="file-transfer-actions">
                  {["paused", "failed"].includes(item.status) && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="icon-btn"
                      disabled={busy.has(item.transfer_id)}
                      aria-label={`${item.status === "paused" ? "继续" : "重试"} ${item.remote_path.split("/").pop()}`}
                      title={
                        item.status === "paused"
                          ? "校验断点并继续"
                          : "从断点重试"
                      }
                      onClick={() =>
                        void control(
                          item,
                          item.status === "paused" ? "resume" : "retry",
                        )
                      }
                    >
                      {item.status === "paused" ? (
                        <Play size={17} />
                      ) : (
                        <RotateCcw size={17} />
                      )}
                    </Button>
                  )}
                  {!terminal(item.status) && item.status !== "paused" && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="icon-btn"
                      disabled={
                        busy.has(item.transfer_id) ||
                        ["committing", "pausing", "cancelling"].includes(
                          item.phase,
                        )
                      }
                      aria-label={`暂停 ${item.remote_path.split("/").pop()}`}
                      title="暂停并保存断点"
                      onClick={() => void control(item, "pause")}
                    >
                      <Pause size={17} />
                    </Button>
                  )}
                  {item.status === "completed" && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="icon-btn"
                      aria-label={`打开 ${item.remote_path.split("/").pop() || item.remote_path} 所在文件夹`}
                      title="打开所在文件夹"
                      onClick={() => void reveal(item)}
                    >
                      <FolderOpen size={18} />
                    </Button>
                  )}
                  {terminal(item.status) ? (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="icon-btn"
                      aria-label={`从列表中移除 ${item.remote_path.split("/").pop() || item.remote_path}`}
                      title="从列表中移除"
                      onClick={() =>
                        setDismissed((current) =>
                          new Set(current).add(item.transfer_id),
                        )
                      }
                    >
                      <Trash2 size={17} />
                    </Button>
                  ) : (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="icon-btn"
                      aria-label={`取消 ${item.remote_path.split("/").pop() || item.remote_path}`}
                      title="取消传输"
                      disabled={["committing", "cancelling"].includes(
                        item.phase,
                      )}
                      onClick={() =>
                        void api<FileTransfer & { cancel_requested: boolean }>(
                          "transfer",
                          {
                            serverId,
                            action: "cancel",
                            transferId: item.transfer_id,
                          },
                        )
                          .then((result) => {
                            setItems((current) =>
                              current.map((x) =>
                                x.transfer_id === result.transfer_id
                                  ? result
                                  : x,
                              ),
                            );
                            if (!result.cancel_requested)
                              callbacks.current.notify(
                                "文件已结束或正在提交，请等待结果",
                              );
                          })
                          .catch((e) =>
                            callbacks.current.notify(String(e), true),
                          )
                      }
                    >
                      <Trash2 size={17} />
                    </Button>
                  )}
                </div>
              </div>
              {item.status !== "completed" && (
                <>
                  <progress
                    aria-label={`${item.direction === "upload" ? "上传" : "下载"} ${item.remote_path}`}
                    max={item.total_bytes || 1}
                    value={item.transferred_bytes}
                  />
                  <div className="file-transfer-description">
                    <span>
                      {formatBytes(item.transferred_bytes)} /{" "}
                      {item.total_bytes === null
                        ? "等待文件大小"
                        : formatBytes(item.total_bytes)}
                      {item.bytes_per_second > 0
                        ? ` · ${formatBytes(item.bytes_per_second)}/s`
                        : ""}
                      {item.bytes_per_second > 0 &&
                      item.total_bytes !== null &&
                      item.status === "running"
                        ? ` · 剩余约 ${Math.ceil(Math.max(0, item.total_bytes - item.transferred_bytes) / item.bytes_per_second)} 秒`
                        : ""}
                    </span>
                    {item.status === "failed" && (
                      <XCircle className="transfer-status-error" size={16} />
                    )}
                    {item.status === "cancelled" && <CheckCircle2 size={16} />}
                    {!terminal(item.status) && item.status !== "paused" && (
                      <LoaderCircle
                        className="spinning transfer-status-active"
                        size={16}
                      />
                    )}
                  </div>
                </>
              )}
              {item.error && (
                <p className="error-inline">{item.error.message}</p>
              )}
              {!!item.retry_count && (
                <small>
                  已自动重试 {item.retry_count} 次
                  {item.next_retry_at
                    ? ` · 下次 ${new Date(item.next_retry_at).toLocaleTimeString()}`
                    : ""}
                </small>
              )}
              {!!item.resumed_bytes && (
                <small>已复用 {formatBytes(item.resumed_bytes)} 断点数据</small>
              )}
              {item.group_id && (
                <small title={item.group_id}>
                  批次 {item.group_id.slice(0, 8)}
                </small>
              )}
              {item.sha256 && (
                <small title={item.sha256}>
                  SHA-256 {item.sha256.slice(0, 16)}…
                </small>
              )}
              {item.status !== "completed" && (
                <details className="transfer-task-options">
                  <summary>传输设置</summary>
                  <label>
                    优先级{" "}
                    <select
                      aria-label={`优先级 ${item.transfer_id}`}
                      value={item.priority || 0}
                      disabled={busy.has(item.transfer_id)}
                      onChange={(e) =>
                        void control(item, "configure", {
                          priority: Number(e.target.value),
                        })
                      }
                    >
                      {[...new Set([-10, -5, 0, 5, 10, item.priority || 0])]
                        .sort((a, b) => a - b)
                        .map((n) => (
                          <option key={n} value={n}>
                            {n}
                          </option>
                        ))}
                    </select>
                  </label>
                  <label>
                    限速 KiB/s（0 跟随全局设置）{" "}
                    <Input
                      type="number"
                      min={0}
                      max={1048576}
                      aria-label={`限速 ${item.transfer_id}`}
                      key={`${item.transfer_id}-${item.bandwidth_kib || 0}`}
                      defaultValue={item.bandwidth_kib || 0}
                      onBlur={(e) => {
                        const n = Number(e.target.value);
                        if (
                          Number.isInteger(n) &&
                          n >= 0 &&
                          n <= 1048576 &&
                          n !== (item.bandwidth_kib || 0)
                        )
                          void control(item, "configure", { bandwidthKib: n });
                      }}
                    />
                  </label>
                  {(terminal(item.status) || item.status === "paused") && (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy.has(item.transfer_id)}
                      onClick={() => void control(item, "cleanup")}
                    >
                      清理临时文件及记录
                    </Button>
                  )}
                </details>
              )}
              {item.audit_error && (
                <p className="error-inline">
                  文件结果已确认，但操作记录保存失败：{item.audit_error}
                </p>
              )}
            </div>
          ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
