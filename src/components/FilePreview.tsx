import { Download, Pencil, X } from "lucide-react";
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
        {data.kind === "text" && (
          <textarea
            className="file-preview-text"
            aria-label="文件预览"
            readOnly
            spellCheck={false}
            value={data.content || ""}
          />
        )}
      </div>
    </ModalFrame>
  );
}
