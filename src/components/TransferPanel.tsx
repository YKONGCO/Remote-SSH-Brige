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
    (item) => !terminal(item.status),
  ).length;
  const latestCompleted = items.find(
    (item) => item.status === "completed" && !dismissed.has(item.transfer_id),
  );
  const reveal = (item: FileTransfer) =>
    api("reveal_transfer", {
      serverId,
      transferId: item.transfer_id,
    }).catch((e) => callbacks.current.notify(String(e), true));
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
          <strong>下载</strong>
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
            {search ? "没有匹配的下载任务" : "暂无下载任务"}
          </div>
        )}
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
                    </span>
                    {item.status === "failed" && (
                      <XCircle className="transfer-status-error" size={16} />
                    )}
                    {item.status === "cancelled" && <CheckCircle2 size={16} />}
                    {!terminal(item.status) && (
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
