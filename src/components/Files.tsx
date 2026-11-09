import { usePreferences } from "../preferences";
import { CompactTime } from "./CompactTime";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Input } from "./ui/input";
import { Button } from "./ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
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
  ChevronRight,
  ChevronDown,
  Files as FilesIcon,
  Search,
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
    data?: Preview;
    loading: boolean;
    error: string;
  } | null>(null);
  const [security, setSecurity] = useState<RuntimeSettings>();
  const [path, setPath] = useState(server?.root || "/"),
    [input, setInput] = useState(path),
    [entries, setEntries] = useState<FileEntry[]>([]),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [filter, setFilter] = useState("");
  const [saving, setSaving] = useState(false);
  const previewRequest = useRef(0);
  const serverRequest = useRef(0);
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
    ++previewRequest.current;
    const securityRequest = ++serverRequest.current;
    setPreview(null);
    setSecurity(undefined);
    if (!server) return;
    void api<RuntimeSettings>("runtime_settings")
      .then((settings) => {
        if (serverRequest.current === securityRequest) setSecurity(settings);
      })
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
      ++previewRequest.current;
      ++serverRequest.current;
    },
    [],
  );
  const join = (name: string) => (path === "/" ? "" : path) + "/" + name;
  const act = async (kind: string, extra: Record<string, string>) => {
    if (!server) return;
    await operate({ kind, server_id: server.id, ...extra });
    refresh();
  };
  const open = async (entry: FileEntry) => {
    if (entry.is_dir) {
      void load(entry.path);
      return;
    }
    if (!server) return;
    const request = ++previewRequest.current;
    setPreview({ entry, loading: true, error: "" });
    try {
      const data = await operate<Preview>({
        kind: "read_file",
        server_id: server.id,
        path: entry.path,
        file_request: { type: "preview" },
      });
      if (request !== previewRequest.current) return;
      setPreview({ entry, data, loading: false, error: "" });
      refresh();
    } catch (e) {
      if (request === previewRequest.current)
        setPreview({ entry, loading: false, error: String(e) });
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
        {desktop ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="default"
                size="sm"
                className="btn primary"
                disabled={security?.remote_read_only}
              >
                <Upload size={15} />
                上传
                <ChevronDown size={13} />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-40">
              <DropdownMenuItem
                disabled={security?.remote_read_only}
                onSelect={() => {
                  void api<FileTransfer | null>("upload_file", {
                    serverId: server.id,
                    remoteDir: path,
                  })
                    .then((transfer) => {
                      if (transfer)
                        notify("上传任务已开始，进度见文件传输列表");
                    })
                    .catch((e) => notify(String(e), true));
                }}
              >
                <FileText size={15} />
                上传文件
              </DropdownMenuItem>
              {[false, true].map((directory) => (
                <DropdownMenuItem
                  key={String(directory)}
                  disabled={security?.remote_read_only}
                  onSelect={() =>
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
                  {directory ? <Folder size={15} /> : <FilesIcon size={15} />}
                  {directory ? "上传文件夹" : "批量上传"}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        ) : (
          <Button
            variant="default"
            size="sm"
            className="btn primary"
            disabled={security?.remote_read_only}
            onClick={() => upload.current?.click()}
          >
            <Upload size={15} />
            上传
          </Button>
        )}
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
      {preview && (
        <FilePreview
          key={server.id + ":" + preview.entry.path}
          entry={preview.entry}
          data={preview.data}
          loading={preview.loading}
          error={preview.error}
          readOnly={security?.remote_read_only ?? true}
          onClose={() => {
            ++previewRequest.current;
            setPreview(null);
          }}
          onRetry={() => void open(preview.entry)}
          onLoadFull={async () => {
            const request = previewRequest.current;
            const result = await operate<{ content: string }>({
              kind: "read_file",
              server_id: server.id,
              path: preview.entry.path,
            });
            if (request !== previewRequest.current)
              throw new Error("文件已关闭或服务器已切换");
            if (
              typeof result.content !== "string" ||
              result.content.includes("\0")
            )
              throw new Error("文件不是 UTF-8 文本，请下载后查看");
            const data: Preview = {
              kind: "text",
              content: result.content,
              bytes: new TextEncoder().encode(result.content).length,
              truncated: false,
            };
            setPreview({ ...preview, data });
            refresh();
            return data;
          }}
          onSave={async (content) => {
            const request = previewRequest.current;
            await operate({
              kind: "write_file",
              server_id: server.id,
              path: preview.entry.path,
              content,
            });
            if (request === previewRequest.current) {
              notify("文件已保存");
              refresh();
              void load(path, true);
            }
          }}
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
