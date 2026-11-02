import { useEffect, useRef, useState } from "react";
import { api, desktop } from "../api";
import { formatBytes, type FileTransfer } from "../transfers";
import { Button } from "./ui/button";

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
  const callbacks = useRef({ onComplete, notify });
  callbacks.current = { onComplete, notify };
  useEffect(() => {
    if (!desktop) return;
    let disposed = false,
      loading = false;
    const known = new Map<string, string>();
    setItems([]);
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
  if (!items.length && !error) return null;
  return (
    <section className="file-transfer-panel" aria-label="文件传输">
      <strong>文件传输</strong>
      {error && <p role="alert">{error}</p>}
      {[
        ...items.filter((item) => !terminal(item.status)),
        ...items.filter((item) => terminal(item.status)),
      ]
        .slice(0, 10)
        .map((item) => (
          <div className="file-transfer-item" key={item.transfer_id}>
            <div className="file-transfer-description">
              <span>
                {item.direction === "upload" ? "上传" : "下载"} ·{" "}
                {item.remote_path}
              </span>
              <span>{phases[item.phase] || item.phase}</span>
            </div>
            <progress
              aria-label={`${item.direction === "upload" ? "上传" : "下载"} ${item.remote_path}`}
              max={item.total_bytes || 1}
              value={
                item.status === "completed"
                  ? item.total_bytes || 1
                  : item.transferred_bytes
              }
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
              {!terminal(item.status) && (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={["committing", "cancelling"].includes(item.phase)}
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
                            x.transfer_id === result.transfer_id ? result : x,
                          ),
                        );
                        if (!result.cancel_requested)
                          callbacks.current.notify(
                            "文件已结束或正在提交，请等待结果",
                          );
                      })
                      .catch((e) => callbacks.current.notify(String(e), true))
                  }
                >
                  取消
                </Button>
              )}
            </div>
            {item.error && <p className="error-inline">{item.error.message}</p>}
            {item.audit_error && (
              <p className="error-inline">
                文件结果已确认，但操作记录保存失败：{item.audit_error}
              </p>
            )}
          </div>
        ))}
    </section>
  );
}
