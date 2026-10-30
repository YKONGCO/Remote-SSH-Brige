import { Download, Pencil, X } from "lucide-react";
import { useState } from "react";
import type { FileEntry, FilePreview as Preview } from "../types";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";

export function FilePreview({
  entry,
  data,
  onClose,
  onEdit,
  onDownload,
  readOnly,
}: {
  entry: FileEntry;
  data: Preview;
  onClose: () => void;
  onEdit: () => void;
  onDownload: () => void;
  readOnly: boolean;
}) {
  const [imageFailed, setImageFailed] = useState(false);
  const image =
    data.kind === "image" &&
    ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(
      data.mime || "",
    );
  return (
    <ModalFrame width={900} className="file-preview-modal" onClose={onClose}>
      <div className="modal-head file-preview-head">
        <DialogTitle asChild>
          <h2 title={entry.path}>{entry.name}</h2>
        </DialogTitle>
        <span className="file-preview-size">
          {data.bytes.toLocaleString()} B
        </span>
        <Button variant="outline" className="btn small" onClick={onDownload}>
          <Download size={14} />
          下载
        </Button>
        {data.kind === "text" && !readOnly && (
          <Button variant="outline" className="btn small" onClick={onEdit}>
            <Pencil size={14} />
            编辑
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon"
          className="icon-btn"
          aria-label="关闭文件预览"
          onClick={onClose}
        >
          <X size={16} />
        </Button>
      </div>
      {data.truncated && (
        <p className="file-preview-notice">
          文件较大，仅预览前{" "}
          {(data.preview_limit_bytes || 2 * 1024 * 1024) / 1024 / 1024}{" "}
          MiB，编辑前会重新读取完整内容。
        </p>
      )}
      <div className="file-preview-body">
        {data.kind === "text" ? (
          <textarea
            className="file-preview-text"
            aria-label="文件预览"
            readOnly
            spellCheck={false}
            value={data.content || ""}
          />
        ) : image && !imageFailed ? (
          <img
            className="file-preview-image"
            alt={entry.name}
            src={`data:${data.mime};base64,${data.base64}`}
            onError={() => setImageFailed(true)}
          />
        ) : (
          <div className="empty-panel">
            <h3>此文件暂不支持预览</h3>
            <p>可下载后使用本地应用查看。</p>
          </div>
        )}
      </div>
    </ModalFrame>
  );
}
