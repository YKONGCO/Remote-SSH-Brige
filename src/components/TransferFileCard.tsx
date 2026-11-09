import { useState } from "react";
import {
  CheckCircle2,
  FileDown,
  FileUp,
  FolderOpen,
  LoaderCircle,
  Pause,
  Play,
  RotateCcw,
  SlidersHorizontal,
  Trash2,
} from "lucide-react";
import { formatBytes, type FileTransfer } from "../transfers";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

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

export function TransferFileCard({
  item,
  nested,
  busy,
  onControl,
  onReveal,
  onDismiss,
}: {
  item: FileTransfer;
  nested?: boolean;
  busy: boolean;
  onControl: (
    item: FileTransfer,
    action: string,
    patch?: Record<string, number>,
  ) => void;
  onReveal: (item: FileTransfer) => void;
  onDismiss: (item: FileTransfer) => void;
}) {
  const [optionsOpen, setOptionsOpen] = useState(false);
  const name = item.remote_path.split("/").pop() || item.remote_path;
  const finished = ["completed", "failed", "cancelled"].includes(item.status);
  const direction = item.direction === "upload" ? "上传" : "下载";
  const progress =
    item.status === "completed"
      ? formatBytes(item.transferred_bytes)
      : `${formatBytes(item.transferred_bytes)} / ${item.total_bytes === null ? "等待文件大小" : formatBytes(item.total_bytes)}${item.bytes_per_second > 0 ? ` · ${formatBytes(item.bytes_per_second)}/s` : ""}`;
  const remaining =
    item.bytes_per_second > 0 &&
    item.total_bytes !== null &&
    item.status === "running"
      ? ` · 剩余约 ${Math.ceil(Math.max(0, item.total_bytes - item.transferred_bytes) / item.bytes_per_second)} 秒`
      : "";
  const description = `${direction} · ${phases[item.phase] || item.phase} · ${progress}${remaining}`;

  return (
    <div
      className={`file-transfer-item${nested ? " is-batch-child" : ""}`}
      data-state={item.status}
    >
      <div className="file-transfer-card-row">
        <span className="transfer-file-symbol" aria-hidden="true">
          {item.direction === "upload" ? (
            <FileUp className="transfer-file-icon" />
          ) : (
            <FileDown className="transfer-file-icon" />
          )}
        </span>
        <div className="file-transfer-copy">
          <span className="file-transfer-name" title={item.remote_path}>
            {name}
          </span>
          <div className="file-transfer-description" title={description}>
            {item.status === "completed" && (
              <CheckCircle2
                className="transfer-status-success"
                size={12}
                aria-hidden="true"
              />
            )}
            {!finished && item.status !== "paused" && (
              <LoaderCircle
                className="spinning transfer-status-active"
                size={12}
                aria-hidden="true"
              />
            )}
            <span>{description}</span>
          </div>
        </div>
        <div className="file-transfer-actions">
          <Button
            variant="ghost"
            size="icon"
            className="icon-btn"
            title={item.status === "completed" ? "传输详情" : "传输设置"}
            aria-label={`${item.status === "completed" ? "传输详情" : "传输设置"} ${name}`}
            aria-expanded={optionsOpen}
            onClick={() => setOptionsOpen(!optionsOpen)}
          >
            <SlidersHorizontal size={14} />
          </Button>
          {["paused", "failed"].includes(item.status) && (
            <Button
              variant="ghost"
              size="icon"
              className="icon-btn"
              disabled={busy}
              aria-label={`${item.status === "paused" ? "继续" : "重试"} ${name}`}
              title={item.status === "paused" ? "校验断点并继续" : "从断点重试"}
              onClick={() =>
                onControl(item, item.status === "paused" ? "resume" : "retry")
              }
            >
              {item.status === "paused" ? (
                <Play size={14} />
              ) : (
                <RotateCcw size={14} />
              )}
            </Button>
          )}
          {!finished && item.status !== "paused" && (
            <Button
              variant="ghost"
              size="icon"
              className="icon-btn"
              disabled={
                busy ||
                ["committing", "pausing", "cancelling"].includes(item.phase)
              }
              aria-label={`暂停 ${name}`}
              title="暂停并保存断点"
              onClick={() => onControl(item, "pause")}
            >
              <Pause size={14} />
            </Button>
          )}
          {item.status === "completed" && (
            <Button
              variant="ghost"
              size="icon"
              className="icon-btn"
              aria-label={`打开 ${name} 所在文件夹`}
              title="打开所在文件夹"
              onClick={() => onReveal(item)}
            >
              <FolderOpen size={15} />
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            className="icon-btn"
            aria-label={`${finished ? "从列表中移除" : "取消"} ${name}`}
            title={finished ? "从列表中移除" : "取消传输"}
            disabled={
              !finished &&
              (busy || ["committing", "cancelling"].includes(item.phase))
            }
            onClick={() =>
              finished ? onDismiss(item) : onControl(item, "cancel")
            }
          >
            <Trash2 size={14} />
          </Button>
        </div>
      </div>
      {!finished && (
        <progress
          aria-label={`${direction} ${item.remote_path}`}
          max={item.total_bytes || 1}
          value={
            item.total_bytes === null
              ? undefined
              : Math.min(item.transferred_bytes, item.total_bytes || 1)
          }
        />
      )}
      {item.error && <p className="error-inline">{item.error.message}</p>}
      {optionsOpen && (
        <div className="transfer-task-options">
          <p className="transfer-task-path" title={item.remote_path}>
            {item.remote_path}
          </p>
          {!!item.retry_count && (
            <p>
              已自动重试 {item.retry_count} 次
              {item.next_retry_at
                ? ` · 下次 ${new Date(item.next_retry_at).toLocaleTimeString()}`
                : ""}
            </p>
          )}
          {!!item.resumed_bytes && (
            <p>已复用 {formatBytes(item.resumed_bytes)} 断点数据</p>
          )}
          {item.sha256 && (
            <p className="transfer-checksum">
              SHA-256 <code>{item.sha256}</code>
            </p>
          )}
          {item.status !== "completed" && (
            <>
              <label>
                优先级
                <select
                  aria-label={`优先级 ${item.transfer_id}`}
                  value={item.priority || 0}
                  disabled={busy}
                  onChange={(event) =>
                    onControl(item, "configure", {
                      priority: Number(event.target.value),
                    })
                  }
                >
                  {[...new Set([-10, -5, 0, 5, 10, item.priority || 0])]
                    .sort((a, b) => a - b)
                    .map((value) => (
                      <option key={value} value={value}>
                        {value}
                      </option>
                    ))}
                </select>
              </label>
              <label>
                限速 KiB/s（0 跟随全局设置）
                <Input
                  type="number"
                  min={0}
                  max={1048576}
                  disabled={busy}
                  aria-label={`限速 ${item.transfer_id}`}
                  key={`${item.transfer_id}-${item.bandwidth_kib || 0}`}
                  defaultValue={item.bandwidth_kib || 0}
                  onBlur={(event) => {
                    const value = Number(event.target.value);
                    if (
                      Number.isInteger(value) &&
                      value >= 0 &&
                      value <= 1048576 &&
                      value !== (item.bandwidth_kib || 0)
                    )
                      onControl(item, "configure", { bandwidthKib: value });
                  }}
                />
              </label>
              {(finished || item.status === "paused") && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => onControl(item, "cleanup")}
                >
                  清理临时文件及记录
                </Button>
              )}
            </>
          )}
        </div>
      )}
      {item.audit_error && (
        <p className="error-inline">
          文件结果已确认，但操作记录保存失败：{item.audit_error}
        </p>
      )}
    </div>
  );
}
