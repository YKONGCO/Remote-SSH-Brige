import { useEffect, useMemo, useRef, useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  Download,
  FolderOpen,
  Layers3,
  LoaderCircle,
  Pause,
  Search,
  XCircle,
} from "lucide-react";
import { api, desktop } from "../api";
import { formatBytes, type FileTransfer } from "../transfers";
import { TransferFileCard } from "./TransferFileCard";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";

const PAGE_SIZE = 30;
const terminal = (status: string) =>
  ["completed", "failed", "cancelled"].includes(status);
const groupStates = {
  completed: { label: "已完成", icon: CheckCircle2 },
  failed: { label: "有失败", icon: XCircle },
  cancelled: { label: "已结束", icon: XCircle },
  paused: { label: "已暂停", icon: Pause },
  active: { label: "进行中", icon: LoaderCircle },
};
type TransferRow =
  | { kind: "batch"; id: string; tasks: FileTransfer[]; expanded: boolean }
  | { kind: "file"; item: FileTransfer; nested: boolean };

function TransferBatchHeader({
  id,
  tasks,
  expanded,
  onToggle,
}: {
  id: string;
  tasks: FileTransfer[];
  expanded: boolean;
  onToggle: () => void;
}) {
  const bytes = tasks.reduce((sum, task) => sum + task.transferred_bytes, 0);
  const total = tasks.reduce((sum, task) => sum + (task.total_bytes || 0), 0);
  const completed = tasks.filter((task) => task.status === "completed").length;
  const ended = tasks.every((task) => terminal(task.status));
  const failed = tasks.some((task) => task.status === "failed");
  const paused =
    !ended &&
    tasks.every((task) => terminal(task.status) || task.status === "paused");
  const state =
    completed === tasks.length
      ? "completed"
      : ended
        ? failed
          ? "failed"
          : "cancelled"
        : paused
          ? "paused"
          : "active";
  const StateIcon = groupStates[state].icon;
  return (
    <div className="transfer-batch" data-state={state}>
      <button
        type="button"
        className="transfer-batch-toggle"
        aria-expanded={expanded}
        aria-label={`批次 ${id}，${groupStates[state].label}，${completed}/${tasks.length} 个文件已完成`}
        title={`批次 ${id} · ${completed}/${tasks.length} 个文件已完成 · ${formatBytes(bytes)} / ${formatBytes(total)}`}
        onClick={onToggle}
      >
        <Layers3 size={15} aria-hidden="true" />
        <span className="transfer-batch-name">批次 {id}</span>
        <span className="transfer-batch-count">
          {completed}/{tasks.length}
        </span>
        <span className="transfer-state-badge">
          <StateIcon
            size={12}
            className={state === "active" ? "spinning" : undefined}
          />
          {groupStates[state].label}
        </span>
        <ChevronDown
          size={14}
          className={expanded ? "is-expanded" : ""}
          aria-hidden="true"
        />
      </button>
      {!ended && (
        <progress
          aria-label={`批次 ${id} 进度`}
          max={total || 1}
          value={
            tasks.every((task) => task.total_bytes !== null)
              ? Math.min(bytes, total || 1)
              : undefined
          }
        />
      )}
    </div>
  );
}

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
  const [expandedGroups, setExpandedGroups] = useState<Map<string, boolean>>(
    new Map(),
  );
  const [menuOpen, setMenuOpen] = useState(false);
  const [renderCount, setRenderCount] = useState(PAGE_SIZE);
  const body = useRef<HTMLDivElement>(null);
  const sentinel = useRef<HTMLDivElement>(null);
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
    setExpandedGroups(new Map());
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

  const availableItems = useMemo(
    () => items.filter((item) => !dismissed.has(item.transfer_id)),
    [items, dismissed],
  );
  const activeCount = availableItems.filter(
    (item) => !terminal(item.status) && item.status !== "paused",
  ).length;
  const rows = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    const matches = (item: FileTransfer) =>
      !query ||
      item.remote_path.toLocaleLowerCase().includes(query) ||
      !!item.group_id?.toLocaleLowerCase().includes(query);
    const ordered = [
      ...availableItems.filter((item) => !terminal(item.status)),
      ...availableItems.filter((item) => terminal(item.status)),
    ];
    const groups = new Map<string, FileTransfer[]>();
    for (const item of ordered) {
      if (!item.group_id) continue;
      const group = groups.get(item.group_id);
      if (group) group.push(item);
      else groups.set(item.group_id, [item]);
    }
    const result: TransferRow[] = [];
    const seen = new Set<string>();
    for (const item of ordered) {
      if (!item.group_id) {
        if (matches(item)) result.push({ kind: "file", item, nested: false });
        continue;
      }
      if (seen.has(item.group_id)) continue;
      seen.add(item.group_id);
      const tasks = groups.get(item.group_id)!;
      const matching = tasks.filter(matches);
      if (!matching.length) continue;
      // Search also reaches files in unopened batches and outside loaded rows.
      const expanded = expandedGroups.get(item.group_id) ?? !!query;
      result.push({ kind: "batch", id: item.group_id, tasks, expanded });
      if (expanded)
        for (const task of matching)
          result.push({ kind: "file", item: task, nested: true });
    }
    return result;
  }, [availableItems, search, expandedGroups]);
  const hasMore = renderCount < rows.length;
  const loadMore = () =>
    setRenderCount((count) => Math.min(rows.length, count + PAGE_SIZE));

  useEffect(() => {
    setRenderCount(PAGE_SIZE);
    setExpandedGroups(new Map());
    if (body.current) body.current.scrollTop = 0;
  }, [serverId, search]);
  useEffect(() => {
    if (!menuOpen || !hasMore || !body.current || !sentinel.current) return;
    const root = body.current;
    if (root.clientHeight > 0 && root.scrollHeight <= root.clientHeight)
      loadMore();
    if (typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) loadMore();
      },
      { root, rootMargin: "120px" },
    );
    observer.observe(sentinel.current);
    return () => observer.disconnect();
  }, [menuOpen, renderCount, rows.length, hasMore]);

  const latestCompleted = availableItems.find(
    (item) => item.status === "completed",
  );
  const reveal = (item: FileTransfer) => {
    void api("reveal_transfer", {
      serverId,
      transferId: item.transfer_id,
    }).catch((e) => callbacks.current.notify(String(e), true));
  };
  const control = async (
    item: FileTransfer,
    action: string,
    patch: Record<string, number> = {},
  ) => {
    setBusy((current) => new Set(current).add(item.transfer_id));
    try {
      const result = await api<FileTransfer & { cancel_requested?: boolean }>(
        "transfer",
        { serverId, action, transferId: item.transfer_id, ...patch },
      );
      if (action === "cleanup")
        setItems((current) =>
          current.filter((task) => task.transfer_id !== item.transfer_id),
        );
      else
        setItems((current) =>
          current.map((task) =>
            task.transfer_id === result.transfer_id ? result : task,
          ),
        );
      if (action === "cancel" && !result.cancel_requested)
        callbacks.current.notify("文件已结束或正在提交，请等待结果");
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
  if (!desktop) return null;
  return (
    <DropdownMenu
      open={menuOpen}
      onOpenChange={(open) => {
        setMenuOpen(open);
        if (open) setRenderCount(PAGE_SIZE);
      }}
    >
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
        aria-labelledby={undefined}
      >
        <div className="transfer-menu-header">
          <strong>
            文件传输
            {activeCount > 0 && (
              <span className="transfer-header-count">
                {activeCount} 项进行中
              </span>
            )}
          </strong>
          <div className="transfer-menu-header-actions">
            <Button
              variant="ghost"
              size="icon"
              className="icon-btn"
              aria-label="打开下载文件夹"
              title="打开下载文件夹"
              disabled={!latestCompleted}
              onClick={() => latestCompleted && reveal(latestCompleted)}
            >
              <FolderOpen size={17} />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="icon-btn"
              aria-label="搜索传输任务"
              title="搜索传输任务"
              aria-pressed={searchOpen}
              onClick={() => setSearchOpen(!searchOpen)}
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
              aria-label="搜索传输任务"
              placeholder="搜索文件名、远程路径或批次"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
          </div>
        )}
        <div
          className="transfer-menu-body"
          ref={body}
          onScroll={(event) => {
            const root = event.currentTarget;
            if (
              hasMore &&
              root.scrollHeight - root.scrollTop - root.clientHeight < 120
            )
              loadMore();
          }}
        >
          {error && (
            <p className="error-inline" role="alert">
              {error}
            </p>
          )}
          {!rows.length && !error && (
            <div className="transfer-menu-empty">
              {search ? "没有匹配的传输任务" : "暂无传输任务"}
            </div>
          )}
          {rows.slice(0, renderCount).map((row, index) =>
            row.kind === "batch" ? (
              <TransferBatchHeader
                key={`batch:${row.id}`}
                id={row.id}
                tasks={row.tasks}
                expanded={row.expanded}
                onToggle={() => {
                  setExpandedGroups((current) =>
                    new Map(current).set(row.id, !row.expanded),
                  );
                  // Expanding the last loaded header should reveal its files immediately.
                  if (!row.expanded && index + 1 === renderCount)
                    setRenderCount((count) => count + PAGE_SIZE);
                }}
              />
            ) : (
              <TransferFileCard
                key={`file:${row.item.transfer_id}`}
                item={row.item}
                nested={row.nested}
                busy={busy.has(row.item.transfer_id)}
                onControl={(item, action, patch) =>
                  void control(item, action, patch)
                }
                onReveal={reveal}
                onDismiss={(item) =>
                  setDismissed((current) =>
                    new Set(current).add(item.transfer_id),
                  )
                }
              />
            ),
          )}
          {hasMore && (
            <div ref={sentinel} className="transfer-load-more" role="status">
              向下滚动加载更多
            </div>
          )}
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
