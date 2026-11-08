import { usePreferences } from "../preferences";
import { CompactTime } from "./CompactTime";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Textarea } from "./ui/textarea";
import { Input } from "./ui/input";
import { Button } from "./ui/button";
import { FilePreview } from "./FilePreview";
import { useEffect, useState, useRef } from "react";
import {
  Folder,
  FileText,
  ArrowUp,
  RefreshCw,
  Upload,
  FolderPlus,
  Plus,
  Download,
  Trash2,
  Pencil,
  Save,
  X,
  ChevronRight,
  Search,
  Code2,
} from "lucide-react";
import { api, desktop, operate } from "../api";
import { TransferPanel } from "./TransferPanel";
import { chooseBrowserSave } from "../save-file";
import type { FileTransfer } from "../transfers";
import { formatBytes } from "../transfers";
import type { RuntimeSettings } from "../runtime-settings";
import type { Server, FileEntry, FilePreview as Preview } from "../types";
export function Files({
  server,
  notify,
  refresh,
}: {
  server?: Server;
  notify: (s: string, error?: boolean) => void;
  refresh: () => void;
}) {
  const preferences = usePreferences();
  const [preview, setPreview] = useState<{
    entry: FileEntry;
    data: Preview;
  } | null>(null);
  const [security, setSecurity] = useState<RuntimeSettings>();
  const [path, setPath] = useState(server?.root || "/"),
    [input, setInput] = useState(path),
    [entries, setEntries] = useState<FileEntry[]>([]),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [filter, setFilter] = useState("");
  const [editor, setEditor] = useState<{
      path: string;
      content: string;
      original: string;
    } | null>(null),
    [saving, setSaving] = useState(false);
  const [dialog, setDialog] = useState<{
    kind: string;
    name: string;
    entry?: FileEntry;
  } | null>(null);
  const downloading = useRef(new Set<string>());
  const upload = useRef<HTMLInputElement>(null);
  const listingRequest = useRef(0);
  const listingBusy = useRef(false);
  const load = async (p = path, quiet = false) => {
    if (!server) return;
    if (quiet && listingBusy.current) return;
    const request = ++listingRequest.current;
    listingBusy.current = true;
    if (!quiet) {
      setBusy(true);
      setError("");
    }
    try {
      const r = await operate<{ path: string; entries: FileEntry[] }>({
        kind: "list_files",
        server_id: server.id,
        path: p,
      });
      if (request !== listingRequest.current) return;
      setEntries(r.entries);
      setError("");
      if (!quiet) {
        setPath(r.path);
        setInput(r.path);
        refresh();
      }
    } catch (e) {
      if (request === listingRequest.current) setError(String(e));
    } finally {
      if (request === listingRequest.current) {
        listingBusy.current = false;
        setBusy(false);
      }
    }
  };
  useEffect(() => {
    ++listingRequest.current;
    listingBusy.current = false;
    setBusy(false);
    setEntries([]);
    setEditor(null);
    setPreview(null);
    if (!server) return;
    void api<RuntimeSettings>("runtime_settings")
      .then(setSecurity)
      .catch(() => {});
    setPath(server?.root || "/");
    setInput(server?.root || "/");
    if (server) void load(server.root);
  }, [server?.id]);
  useEffect(() => {
    if (!server?.enabled || preferences.refreshSeconds === "0") return;
    const timer = setInterval(
      () => void load(path, true),
      Number(preferences.refreshSeconds) * 1000,
    );
    return () => clearInterval(timer);
  }, [server?.id, server?.enabled, path, preferences.refreshSeconds]);
  useEffect(
    () => () => {
      ++listingRequest.current;
    },
    [],
  );
  const join = (name: string) => (path === "/" ? "" : path) + "/" + name;
  const act = async (kind: string, extra: Record<string, string>) => {
    if (!server) return;
    await operate({ kind, server_id: server.id, ...extra });
    refresh();
  };
  const edit = async (e: FileEntry) => {
    if (editor && editor.content !== editor.original) {
      notify("请先保存或关闭当前编辑器", true);
      return;
    }
    if (e.is_dir) {
      void load(e.path);
      return;
    }
    try {
      const r = await operate<{ content: string }>({
        kind: "read_file",
        server_id: server!.id,
        path: e.path,
      });
      setEditor({ path: e.path, content: r.content, original: r.content });
      setPreview(null);
      refresh();
    } catch (e) {
      notify(String(e), true);
    }
  };
  const open = async (entry: FileEntry) => {
    if (entry.is_dir) {
      void load(entry.path);
      return;
    }
    try {
      const result = await operate<{ content: string }>({
        kind: "read_file",
        server_id: server!.id,
        path: entry.path,
      });
      setPreview({
        entry,
        data: {
          kind: "text",
          content: result.content,
          bytes: entry.size,
          truncated: false,
        },
      });
      refresh();
    } catch (e) {
      notify(String(e), true);
    }
  };
  const download = async (entry: FileEntry) => {
    if (downloading.current.has(entry.path)) return;
    downloading.current.add(entry.path);
    try {
      if (!desktop) {
        // Browser preview has only a small in-memory demo, never a real SSH transfer.
        const save = await chooseBrowserSave(entry.name);
        if (!save) return;
        const data = await operate<{ base64: string }>({
          kind: "download",
          server_id: server!.id,
          path: entry.path,
        });
        await save(
          new Blob([
            Uint8Array.from(atob(data.base64), (c) => c.charCodeAt(0)),
          ]),
        );
        notify("文件已保存");
        return;
      }
      const transfer = await api<FileTransfer | null>("download_file", {
        serverId: server!.id,
        path: entry.path,
        name: entry.name,
      });
      if (transfer) notify("下载任务已开始，进度见文件传输列表");
    } catch (e) {
      notify(String(e), true);
    } finally {
      downloading.current.delete(entry.path);
      refresh();
    }
  };
  const visibleEntries = entries.filter(
    (e) =>
      (preferences.showHidden || !e.name.startsWith(".")) &&
      e.name.toLowerCase().includes(filter.toLowerCase()),
  );
  if (!server)
    return (
      <div className="empty-panel">
        <Folder size={38} />
        <h3>选择一台服务器</h3>
        <p>请先在右上方选择服务器，再查看远程文件。</p>
      </div>
    );
  return (
    <div className="files-workspace">
      <div className="toolbar file-toolbar">
        <div className="button-group">
          <Button
            variant="ghost"
            size="icon"
            className="icon-btn"
            title="上级目录"
            onClick={() =>
              void load(path.split("/").slice(0, -1).join("/") || "/")
            }
          >
            <ArrowUp size={17} />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="icon-btn"
            title="刷新目录"
            onClick={() => void load()}
          >
            <RefreshCw size={16} className={busy ? "spinning" : ""} />
          </Button>
        </div>
        <form
          className="path-input"
          onSubmit={(e) => {
            e.preventDefault();
            void load(input);
          }}
        >
          <Folder size={16} />
          <Input
            aria-label="远程目录"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
          <Button
            variant="ghost"
            size="icon"
            className="icon-btn"
            title="打开目录"
          >
            <ChevronRight size={16} />
          </Button>
        </form>
        <div className="inline-search file-filter">
          <Search size={14} />
          <Input
            aria-label="筛选文件"
            placeholder="筛选文件…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <TransferPanel
          serverId={server.id}
          onComplete={() => void load()}
          notify={notify}
        />
        <Button
          variant="outline"
          size="sm"
          className="btn"
          disabled={security?.remote_read_only}
          onClick={() => setDialog({ kind: "mkdir", name: "" })}
        >
          <FolderPlus size={15} />
          新建目录
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="btn"
          disabled={security?.remote_read_only}
          onClick={() => setDialog({ kind: "write_file", name: "" })}
        >
          <Plus size={15} />
          新建文件
        </Button>
        <Button
          variant="default"
          size="sm"
          className="btn primary"
          disabled={security?.remote_read_only}
          onClick={() => {
            if (!desktop) {
              upload.current?.click();
              return;
            }
            void api<FileTransfer | null>("upload_file", {
              serverId: server.id,
              remoteDir: path,
            })
              .then((transfer) => {
                if (transfer) notify("上传任务已开始，进度见文件传输列表");
              })
              .catch((e) => notify(String(e), true));
          }}
        >
          <Upload size={15} />
          上传
        </Button>
        {desktop &&
          [false, true].map((directory) => (
            <Button
              key={String(directory)}
              variant="outline"
              size="sm"
              className="btn"
              disabled={security?.remote_read_only}
              onClick={() =>
                void api<{
                  tasks: unknown[];
                  queued?: number;
                  rejected?: number;
                } | null>("pick_transfer_batch", {
                  serverId: server.id,
                  remotePath: path,
                  direction: "upload",
                  directory,
                })
                  .then((batch) => {
                    if (batch)
                      notify(
                        `已提交 ${batch.queued ?? batch.tasks.length} 个传输任务${batch.rejected ? `，${batch.rejected} 项未能提交` : ""}`,
                        !!batch.rejected,
                      );
                  })
                  .catch((e) => notify(String(e), true))
              }
            >
              {directory ? "上传文件夹" : "批量上传"}
            </Button>
          ))}
        {!desktop && (
          <input
            hidden
            type="file"
            ref={upload}
            onChange={async (e) => {
              const f = e.target.files?.[0];
              if (!f) return;
              try {
                const limits = await api<RuntimeSettings>("runtime_settings");
                if (
                  f.size >
                  Math.min(limits.upload_limit_mib || 256, 256) * 1024 * 1024
                ) {
                  notify(
                    `文件超过 ${limits.upload_limit_mib} MiB 上传限制`,
                    true,
                  );
                  return;
                }
                if (entries.some((x) => x.name === f.name)) {
                  setDialog({ kind: "upload-conflict", name: f.name });
                  return;
                }
                setBusy(true);
                const bytes = new Uint8Array(await f.arrayBuffer());
                let binary = "";
                for (let i = 0; i < bytes.length; i += 8192)
                  binary += String.fromCharCode(...bytes.subarray(i, i + 8192));
                await act("upload", {
                  path: join(f.name),
                  content: btoa(binary),
                });
                notify("文件已上传");
                await load();
              } catch (e) {
                notify(String(e), true);
              } finally {
                setBusy(false);
                e.target.value = "";
              }
            }}
          />
        )}
      </div>
      {error && (
        <div className="error-inline">
          {error}
          <Button
            variant="outline"
            size="sm"
            className="text-btn"
            onClick={() => void load()}
          >
            重试
          </Button>
        </div>
      )}
      <div className="file-surface">
        <div className="file-list-scroll">
          <table className="data-table file-table">
            <colgroup>
              <col className="file-col-name" />
              <col className="file-col-size" />
              <col className="file-col-permission" />
              <col className="file-col-time" />
              <col className="file-col-actions" />
            </colgroup>
            <thead>
              <tr>
                <th>名称</th>
                <th className="file-col-size">文件大小</th>
                <th className="file-col-permission">权限</th>
                <th>修改时间</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {visibleEntries.map((e) => (
                <tr key={e.path}>
                  <td>
                    <button
                      className="file-name"
                      title={e.name}
                      aria-label={e.name}
                      onClick={() => void open(e)}
                    >
                      {e.is_dir ? (
                        <Folder size={19} className="folder-icon" />
                      ) : (
                        <FileText size={18} className="muted" />
                      )}
                      <span className="file-name-label">{e.name}</span>
                      {e.is_symlink && <span className="tag">链接</span>}
                    </button>
                  </td>
                  <td className="mono muted file-col-size">
                    {e.is_dir ? "—" : formatSize(e.size)}
                  </td>
                  <td className="mono muted file-col-permission">
                    {e.permissions}
                  </td>
                  <td className="muted">
                    <CompactTime value={e.modified * 1000} />
                  </td>
                  <td>
                    <div className="row-actions">
                      {!e.is_dir && (
                        <Button
                          variant="ghost"
                          size="icon"
                          className="icon-btn"
                          title="下载"
                          onClick={() => void download(e)}
                        >
                          <Download size={15} />
                        </Button>
                      )}
                      {desktop && e.is_dir && !e.is_symlink && (
                        <Button
                          variant="ghost"
                          size="icon"
                          className="icon-btn"
                          title="下载文件夹"
                          onClick={() =>
                            void api<{
                              tasks: unknown[];
                              queued?: number;
                              rejected?: number;
                            } | null>("pick_transfer_batch", {
                              serverId: server.id,
                              remotePath: e.path,
                              direction: "download",
                              directory: true,
                            })
                              .then((batch) => {
                                if (batch)
                                  notify(
                                    `已提交 ${batch.queued ?? batch.tasks.length} 个下载任务${batch.rejected ? `，${batch.rejected} 项未能提交` : ""}`,
                                    !!batch.rejected,
                                  );
                              })
                              .catch((err) => notify(String(err), true))
                          }
                        >
                          <Download size={15} />
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        size="icon"
                        className="icon-btn"
                        title="重命名"
                        disabled={security?.remote_read_only}
                        onClick={() =>
                          setDialog({ kind: "rename", name: e.name, entry: e })
                        }
                      >
                        <Pencil size={14} />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="icon-btn danger"
                        title={
                          security?.deletion_protection
                            ? "删除保护已开启"
                            : "删除"
                        }
                        disabled={
                          security?.deletion_protection ||
                          security?.remote_read_only
                        }
                        onClick={() =>
                          setDialog({ kind: "remove", name: e.name, entry: e })
                        }
                      >
                        <Trash2 size={15} />
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {!visibleEntries.length && !busy && !error && (
            <div className="empty-small">这个目录还是空的</div>
          )}
        </div>
      </div>
      {editor && (
        <div className="editor-panel">
          <div className="editor-head">
            <span>
              <Code2 size={17} />
              {editor.path}
              {editor.content !== editor.original && (
                <span className="status-dot orange" />
              )}
            </span>
            <div>
              <Button
                variant="default"
                size="sm"
                className="btn small primary"
                disabled={saving || editor.content === editor.original}
                onClick={async () => {
                  setSaving(true);
                  try {
                    await act("write_file", {
                      path: editor.path,
                      content: editor.content,
                    });
                    setEditor({ ...editor, original: editor.content });
                    notify("文件已保存");
                    void load();
                  } catch (e) {
                    notify(String(e), true);
                  } finally {
                    setSaving(false);
                  }
                }}
              >
                <Save size={14} />
                {saving ? "保存中…" : "保存更改"}
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="icon-btn"
                title="关闭编辑器"
                onClick={() =>
                  editor.content !== editor.original
                    ? setDialog({ kind: "discard", name: "" })
                    : setEditor(null)
                }
              >
                <X size={16} />
              </Button>
            </div>
          </div>
          <Textarea
            aria-label="文件内容"
            className="code-editor"
            spellCheck={false}
            value={editor.content}
            onChange={(e) => setEditor({ ...editor, content: e.target.value })}
          />
          <div className="editor-status">
            UTF-8{" "}
            <span>
              {editor.content.split("\n").length} 行 · {editor.content.length}{" "}
              字符
            </span>
          </div>
        </div>
      )}
      {preview && (
        <FilePreview
          entry={preview.entry}
          data={preview.data}
          readOnly={security?.remote_read_only ?? false}
          onClose={() => setPreview(null)}
          onEdit={() => void edit(preview.entry)}
          onDownload={() => void download(preview.entry)}
        />
      )}
      {dialog && (
        <ModalFrame
          onClose={() => {
            if (!saving) setDialog(null);
          }}
          className="compact-modal"
        >
          <form
            className="modal-form"
            onSubmit={async (e) => {
              e.preventDefault();
              if (dialog.kind === "upload-conflict") {
                setDialog(null);
                return;
              }
              if (dialog.kind === "discard") {
                setEditor(null);
                setDialog(null);
                return;
              }
              const name = dialog.name.trim();
              if (
                dialog.kind !== "remove" &&
                (!name || name === "." || name === ".." || /[\\/\0]/.test(name))
              ) {
                notify("请输入不含路径分隔符的名称", true);
                return;
              }
              setSaving(true);
              try {
                if (dialog.kind === "remove")
                  await act("remove", { path: dialog.entry!.path });
                else if (dialog.kind === "rename")
                  await act("rename", {
                    path: dialog.entry!.path,
                    target: join(name),
                  });
                else await act(dialog.kind, { path: join(name), content: "" });
                setDialog(null);
                notify("操作完成");
                await load();
              } catch (e) {
                notify(String(e), true);
              } finally {
                setSaving(false);
              }
            }}
          >
            <div className="modal-head">
              <DialogTitle asChild>
                <h2>
                  {
                    (
                      {
                        remove: "删除远程文件",
                        rename: "重命名",
                        mkdir: "新建目录",
                        write_file: "新建文件",
                        discard: "放弃未保存的修改？",
                        "upload-conflict": "同名文件已存在",
                      } as Record<string, string>
                    )[dialog.kind]
                  }
                </h2>
              </DialogTitle>
            </div>
            <div className="modal-body">
              {dialog.kind === "remove" ? (
                <p>
                  确定删除「{dialog.name}
                  」？此操作会直接修改远程服务器，目录必须为空。
                </p>
              ) : dialog.kind === "discard" ? (
                <p>编辑器中的修改尚未保存。</p>
              ) : dialog.kind === "upload-conflict" ? (
                <p>请先重命名或删除远程同名文件，再上传「{dialog.name}」。</p>
              ) : (
                <label>
                  名称
                  <Input
                    autoFocus
                    required
                    value={dialog.name}
                    onChange={(e) =>
                      setDialog({ ...dialog, name: e.target.value })
                    }
                  />
                </label>
              )}
            </div>
            <div className="modal-foot">
              <Button
                variant="outline"
                size="sm"
                type="button"
                className="btn"
                onClick={() => setDialog(null)}
              >
                取消
              </Button>
              <Button
                variant={dialog.kind === "remove" ? "destructive" : "default"}
                size="sm"
                className={
                  "btn " +
                  (dialog.kind === "remove" ? "danger-fill" : "primary")
                }
                disabled={saving}
              >
                {saving ? "处理中…" : "确认"}
              </Button>
            </div>
          </form>
        </ModalFrame>
      )}
    </div>
  );
}
export const formatSize = formatBytes;
