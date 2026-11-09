import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import {
  Code2,
  Download,
  Eye,
  LoaderCircle,
  Maximize2,
  Minimize2,
  Pencil,
  Save,
  WrapText,
  X,
} from "lucide-react";
import type { FileEntry, FilePreview as Preview } from "../types";
import type { EditorStats } from "./CodeEditor";
import { formatBytes } from "../transfers";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";

const CodeEditor = lazy(() => import("./CodeEditor"));

export function FilePreview({
  entry,
  data,
  loading,
  error,
  onClose,
  onRetry,
  onLoadFull,
  onSave,
  onDownload,
  readOnly,
}: {
  entry: FileEntry;
  data?: Preview;
  loading: boolean;
  error: string;
  onClose: () => void;
  onRetry: () => void;
  onLoadFull: () => Promise<Preview>;
  onSave: (content: string) => Promise<void>;
  onDownload: () => void;
  readOnly: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [content, setContent] = useState(data?.content || "");
  const [original, setOriginal] = useState(content);
  const [saving, setSaving] = useState(false);
  const saveBusy = useRef(false);
  const editorEscape = useRef<() => boolean>(() => false);
  const [loadingFull, setLoadingFull] = useState(false);
  const [localError, setLocalError] = useState("");
  const [confirmClose, setConfirmClose] = useState(false);
  const [wrap, setWrap] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [language, setLanguage] = useState("纯文本");
  const [stats, setStats] = useState<EditorStats>({
    lines: 1,
    line: 1,
    column: 1,
  });
  const dirty = content !== original;
  const lineEnding = useMemo(
    () =>
      data?.content?.includes("\r\n")
        ? "CRLF"
        : data?.content?.includes("\r")
          ? "CR"
          : "LF",
    [data],
  );

  useEffect(() => {
    setContent(data?.content || "");
    setOriginal(data?.content || "");
  }, [data]);

  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  const save = async (close = false) => {
    if (
      saveBusy.current ||
      !data ||
      data.truncated ||
      readOnly ||
      !dirty ||
      loadingFull
    )
      return;
    saveBusy.current = true;
    setSaving(true);
    setLocalError("");
    const saved = content;
    try {
      await onSave(saved);
      setOriginal(saved);
      setConfirmClose(false);
      if (close) onClose();
    } catch (e) {
      setLocalError(String(e));
    } finally {
      saveBusy.current = false;
      setSaving(false);
    }
  };
  const edit = async () => {
    if (!data || readOnly || loadingFull || saving) return;
    setLocalError("");
    if (data.truncated) {
      setLoadingFull(true);
      try {
        const full = await onLoadFull();
        if (full.truncated)
          throw new Error("文件内容不完整，无法编辑，请下载后修改。");
        setContent(full.content || "");
        setOriginal(full.content || "");
        setEditing(true);
      } catch (e) {
        setLocalError(String(e));
      } finally {
        setLoadingFull(false);
      }
    } else setEditing(true);
  };
  const close = () => {
    if (saving) return;
    if (dirty) setConfirmClose(true);
    else onClose();
  };

  return (
    <ModalFrame
      width={expanded ? 100000 : 1120}
      className={`file-preview-modal${expanded ? " file-preview-expanded" : ""}`}
      onClose={close}
      onEscapeKeyDown={(event) => {
        if (editorEscape.current()) event.preventDefault();
      }}
    >
      <div className="modal-head file-preview-head">
        <span className="file-preview-icon">
          <Code2 size={21} />
        </span>
        <div className="file-preview-heading">
          <DialogTitle asChild>
            <h2>
              {entry.name}
              {dirty && (
                <span className="file-unsaved-dot" title="有未保存的修改" />
              )}
            </h2>
          </DialogTitle>
          <p title={entry.path}>{entry.path}</p>
        </div>
        <span className="file-preview-size">
          {formatBytes(data?.bytes ?? entry.size)}
        </span>
        <Button variant="outline" size="sm" onClick={onDownload}>
          <Download size={14} />
          下载
        </Button>
        <Button
          variant="ghost"
          size="icon"
          aria-label={expanded ? "还原窗口" : "放大窗口"}
          onClick={() => setExpanded(!expanded)}
        >
          {expanded ? <Minimize2 size={16} /> : <Maximize2 size={16} />}
        </Button>
        <Button
          variant="ghost"
          size="icon"
          aria-label="关闭文件预览"
          disabled={saving}
          onClick={close}
        >
          <X size={17} />
        </Button>
      </div>
      <div className="file-preview-toolbar">
        <div className="file-view-switch" role="group" aria-label="查看模式">
          <Button
            variant={!editing ? "secondary" : "ghost"}
            size="sm"
            aria-pressed={!editing}
            disabled={saving || loadingFull}
            onClick={() => setEditing(false)}
          >
            <Eye size={14} />
            预览
          </Button>
          {!readOnly && (
            <Button
              variant={editing ? "secondary" : "ghost"}
              size="sm"
              aria-pressed={editing}
              disabled={!data || loading || loadingFull || saving}
              onClick={() => void edit()}
            >
              {loadingFull ? (
                <LoaderCircle size={14} className="spinning" />
              ) : (
                <Pencil size={14} />
              )}
              编辑
            </Button>
          )}
        </div>
        <span className="file-mode-label">
          {readOnly
            ? "只读模式"
            : dirty
              ? "有未保存的修改"
              : editing
                ? "正在编辑远程文件"
                : "只读预览"}
        </span>
        <Button
          variant="ghost"
          size="sm"
          aria-pressed={wrap}
          disabled={!data}
          onClick={() => setWrap(!wrap)}
        >
          <WrapText size={15} />
          自动换行
        </Button>
        {!readOnly && (
          <Button
            variant="default"
            size="sm"
            disabled={
              !dirty || saving || loadingFull || !data || data.truncated
            }
            onClick={() => void save()}
          >
            {saving ? (
              <LoaderCircle size={14} className="spinning" />
            ) : (
              <Save size={14} />
            )}
            {saving ? "保存中…" : "保存更改"}
          </Button>
        )}
      </div>
      {data?.truncated && (
        <p className="file-preview-notice">
          文件较大，仅预览前{" "}
          {formatBytes(data.preview_limit_bytes ?? 20 * 1024 * 1024)}。
          {readOnly
            ? "下载可查看完整内容。"
            : "点击编辑将先加载完整内容，加载成功后才可保存。"}
        </p>
      )}
      {(error || localError) && (
        <div className="file-preview-error" role="alert">
          <span>{localError || error}</span>
          {!data && !loading && (
            <Button variant="outline" size="sm" onClick={onRetry}>
              重试
            </Button>
          )}
        </div>
      )}
      <div className="file-preview-body" aria-busy={loading || loadingFull}>
        {loading ? (
          <div className="file-preview-placeholder" role="status">
            <LoaderCircle size={24} className="spinning" />
            <span>正在读取远程文件…</span>
          </div>
        ) : data ? (
          <Suspense
            fallback={
              <div className="file-preview-placeholder" role="status">
                正在加载代码视图…
              </div>
            }
          >
            <CodeEditor
              filename={entry.path}
              value={content}
              readOnly={
                !editing || readOnly || saving || loadingFull || confirmClose
              }
              wrap={wrap}
              onChange={setContent}
              onSave={() => void save()}
              onStats={setStats}
              onLanguage={setLanguage}
              onEscapeReady={(handler) => {
                editorEscape.current = handler;
              }}
            />
          </Suspense>
        ) : (
          <div className="file-preview-placeholder">
            <Code2 size={30} />
            <span>暂时无法预览此文件，可重试或下载后查看。</span>
          </div>
        )}
      </div>
      {confirmClose && (
        <div className="file-close-confirm" role="alert">
          <div>
            <strong>修改尚未保存</strong>
            <p>关闭前保存更改，或放弃本次修改。</p>
          </div>
          <div className="file-close-actions">
            <Button
              variant="outline"
              size="sm"
              disabled={saving}
              onClick={() => setConfirmClose(false)}
            >
              继续编辑
            </Button>
            <Button
              variant="destructive"
              size="sm"
              disabled={saving}
              onClick={onClose}
            >
              放弃修改
            </Button>
            <Button
              size="sm"
              disabled={saving || readOnly}
              onClick={() => void save(true)}
            >
              保存并关闭
            </Button>
          </div>
        </div>
      )}
      <div className="file-preview-status">
        <span>
          {language} · UTF-8 · {lineEnding}
        </span>
        <span>
          {stats.lines.toLocaleString()} 行 · 第 {stats.line} 行，第{" "}
          {stats.column} 列
        </span>
        <span>Ctrl/⌘ F 搜索{!readOnly && " · Ctrl/⌘ S 保存"}</span>
      </div>
    </ModalFrame>
  );
}
