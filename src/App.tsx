import { productName } from "./branding";
import { McpPanel } from "./components/McpPanel";
import { Settings } from "./components/Settings";
import { ApprovalCenter } from "./components/ApprovalCenter";
import { readPreferences, usePreferences } from "./preferences";
import { CompactTime } from "./components/CompactTime";
import { AuditDetails } from "./components/AuditDetails";
import { AuditTimeFilter } from "./components/AuditTimeFilter";
import { Badge } from "./components/ui/badge";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "./components/ui/dropdown-menu";
import { SelectField } from "./components/SelectField";
import { Tabs, TabsList, TabsTrigger } from "./components/ui/tabs";
import { ModalFrame, DialogTitle } from "./components/ModalFrame";
import { Input } from "./components/ui/input";
import { Button } from "./components/ui/button";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Server as ServerIcon,
  FolderOpen,
  TerminalSquare,
  Activity,
  Settings2,
  Plug,
  Plus,
  Search,
  ChevronDown,
  MoreHorizontal,
  RefreshCw,
  ShieldCheck,
  Play,
  CheckCircle2,
  AlertTriangle,
  X,
  Cloud,
  Box,
  Database,
  FlaskConical,
  Power,
  Pencil,
  Trash2,
  Download,
  LoaderCircle,
} from "lucide-react";
import { api, desktop, operate } from "./api";
import { newServer, labels } from "./types";
import type {
  Server,
  Snapshot,
  Audit,
  AuditQuery,
  AuditPage,
  LoginPathUpdate,
} from "./types";
import { ServerModal } from "./components/ServerModal";
import { mcpPermissionSummary } from "./mcp-permissions";
import appIcon from "./assets/app-icon.svg";
import { Files } from "./components/Files";
import { Jobs } from "./components/Jobs";
import { PortForwards } from "./components/PortForwards";
const Terminals = lazy(() =>
  import("./components/Terminal").then((m) => ({ default: m.Terminals })),
);
import { WindowBar } from "./components/WindowBar";

type Page =
  | "servers"
  | "files"
  | "terminals"
  | "jobs"
  | "tunnels"
  | "audit"
  | "mcp"
  | "settings";
const navigation = [
  { id: "servers", label: "服务器", icon: ServerIcon },
  { id: "files", label: "远程文件", icon: FolderOpen },
  { id: "terminals", label: "终端会话", icon: TerminalSquare },
  { id: "tunnels", label: "端口转发", icon: Plug },
  { id: "jobs", label: "后台任务", icon: Play },
  { id: "audit", label: "操作记录", icon: Activity },
] as const;
const titles: Record<Page, string> = {
  servers: "服务器",
  files: "远程文件",
  terminals: "终端会话",
  jobs: "后台任务",
  tunnels: "端口转发",
  audit: "操作记录",
  mcp: "MCP 服务",
  settings: "设置",
};
const empty: Snapshot = {
  servers: [],
  audits: [],
  terminals: [],
  mcp: { running: false, port: 18920, token: "" },
  data_dir: "",
};
function serverIcon(s: Server) {
  return s.color === "purple"
    ? Database
    : s.color === "green"
      ? FlaskConical
      : s.color === "orange"
        ? Cloud
        : Box;
}
function auditSucceeded(entry: Audit) {
  return entry.kind !== "security_reject" && entry.success;
}
export default function App() {
  const preferences = usePreferences();
  const [titlebarActions, setTitlebarActions] = useState<HTMLDivElement | null>(
    null,
  );
  useEffect(() => {
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme =
        preferences.theme === "system"
          ? media?.matches
            ? "dark"
            : "light"
          : preferences.theme;
      document.documentElement.dataset.density = preferences.density;
      document.documentElement.dataset.motion = String(preferences.motion);
    };
    apply();
    media?.addEventListener("change", apply);
    return () => media?.removeEventListener("change", apply);
  }, [preferences.theme, preferences.density, preferences.motion]);
  useEffect(() => {
    const disableContextMenu = (event: MouseEvent) => event.preventDefault();
    document.addEventListener("contextmenu", disableContextMenu);
    return () =>
      document.removeEventListener("contextmenu", disableContextMenu);
  }, []);
  const [data, setData] = useState<Snapshot>(empty),
    [page, setPage] = useState<Page>(() => readPreferences().startPage as Page),
    [search, setSearch] = useState(""),
    [group, setGroup] = useState("全部服务器"),
    [selectedId, setSelectedId] = useState(""),
    [fileServerId, setFileServerId] = useState(""),
    [modal, setModal] = useState<Server | null>(null),
    [menu, setMenu] = useState(""),
    [toast, setToast] = useState<{ message: string; error: boolean } | null>(
      null,
    ),
    [checking, setChecking] = useState(""),
    [trust, setTrust] = useState<{
      server: Server;
      fingerprint: string;
    } | null>(null),
    [deleting, setDeleting] = useState<Server | null>(null),
    [tab, setTab] = useState("all"),
    [auditSearch, setAuditSearch] = useState(""),
    [auditDetail, setAuditDetail] = useState<Audit | null>(null),
    [port, setPort] = useState(18920),
    [mcpHost, setMcpHost] = useState("127.0.0.1"),
    [mcpBusy, setMcpBusy] = useState(false),
    [loadError, setLoadError] = useState("");
  const notify = useCallback(
    (message: string, error = false) => setToast({ message, error }),
    [],
  );
  const refresh = useCallback(async () => {
    try {
      setData(await api<Snapshot>("snapshot"));
      setLoadError("");
    } catch (e) {
      setLoadError(String(e));
    }
  }, []);
  useEffect(() => {
    void refresh();
    if (preferences.refreshSeconds === "0") return;
    const timer = setInterval(
      () => void refresh(),
      Number(preferences.refreshSeconds) * 1000,
    );
    return () => clearInterval(timer);
  }, [refresh, preferences.refreshSeconds]);
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 5000);
    return () => clearTimeout(timer);
  }, [toast]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setMenu("");
        setTrust(null);
        setDeleting(null);
        setAuditDetail(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const selected =
    data.servers.find((s) => s.id === selectedId) ||
    data.servers.find((s) => s.enabled) ||
    data.servers[0];
  const enabled = data.servers.filter((s) => s.enabled);
  const fileServer = enabled.find((s) => s.id === fileServerId);
  const groups = Array.from(new Set(data.servers.map((s) => s.group)));
  const visible = data.servers.filter(
    (s) =>
      (group === "全部服务器" || s.group === group) &&
      (s.name + " " + s.host + " " + s.username)
        .toLowerCase()
        .includes(search.toLowerCase()),
  );
  const [auditQuery, setAuditQuery] = useState<AuditQuery>({
    category: "all",
    search: "",
    offset: 0,
    limit: 50,
  });
  const [auditPage, setAuditPage] = useState<AuditPage>({
    entries: [],
    total: 0,
    upper: 0,
  });
  const [auditLoading, setAuditLoading] = useState(false);
  const [auditError, setAuditError] = useState("");
  const [exporting, setExporting] = useState(false);
  const auditScroll = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const timer = setTimeout(
      () =>
        setAuditQuery((query) =>
          query.search === auditSearch
            ? query
            : { ...query, search: auditSearch, offset: 0, upper: undefined },
        ),
      250,
    );
    return () => clearTimeout(timer);
  }, [auditSearch]);
  useEffect(() => {
    if (page !== "audit") return;
    let active = true;
    let inFlight = false;
    let loaded = false;
    const load = async () => {
      if (inFlight) return;
      inFlight = true;
      if (!loaded) setAuditLoading(true);
      setAuditError("");
      try {
        const result = await api<AuditPage>("audit_page", {
          query: auditQuery,
        });
        if (active) {
          // Keep the previous rows visible until the replacement is ready.
          if (!loaded && auditQuery.offset === 0 && auditScroll.current) {
            auditScroll.current.scrollTop = 0;
          }
          setAuditPage((previous) => {
            if (auditQuery.offset === 0) return result;
            const existingIds = new Set(
              previous.entries.map((entry) => entry.id),
            );
            return {
              ...result,
              entries: [
                ...previous.entries,
                ...result.entries.filter((entry) => !existingIds.has(entry.id)),
              ],
            };
          });
        }
      } catch (error) {
        if (active) setAuditError(String(error));
      } finally {
        inFlight = false;
        loaded = true;
        if (active) setAuditLoading(false);
      }
    };
    void load();
    const timer =
      preferences.refreshSeconds === "0" || auditQuery.offset > 0
        ? undefined
        : setInterval(
            () => void load(),
            Number(preferences.refreshSeconds) * 1000,
          );
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [page, auditQuery, preferences.refreshSeconds]);
  const filteredAudits = auditPage.entries;
  const auditItemKey = useCallback(
    (index: number) => filteredAudits[index].id,
    [filteredAudits],
  );
  const auditRows = useVirtualizer({
    count: filteredAudits.length,
    getScrollElement: () => auditScroll.current,
    getItemKey: auditItemKey,
    estimateSize: () => 88,
    measureElement: (element) => element.getBoundingClientRect().height || 88,
    overscan: 8,
    enabled: page === "audit",
  });
  const virtualAuditRows = auditRows.getVirtualItems();
  const loadMoreAudits = useCallback(() => {
    const container = auditScroll.current;
    if (
      !container ||
      container.clientHeight === 0 ||
      auditLoading ||
      auditError ||
      !auditPage.entries.length ||
      auditPage.entries.length >= auditPage.total ||
      container.scrollHeight - container.scrollTop - container.clientHeight >
        200
    )
      return;
    setAuditQuery((query) =>
      query.offset > 0 && query.offset >= auditPage.entries.length
        ? query
        : {
            ...query,
            offset: auditPage.entries.length,
            upper: auditPage.upper,
          },
    );
  }, [auditLoading, auditError, auditPage]);
  useEffect(() => {
    if (page !== "audit" || !auditScroll.current) return;
    const observer = new ResizeObserver(loadMoreAudits);
    observer.observe(auditScroll.current);
    loadMoreAudits();
    return () => observer.disconnect();
  }, [page, loadMoreAudits]);
  const exportAudits = async () => {
    setExporting(true);
    try {
      const saved = await api<boolean>("export_audits", {
        query: {
          ...auditQuery,
          category: tab,
          search: auditSearch,
          upper: undefined,
        },
      });
      if (saved) notify("操作记录已导出");
    } catch (error) {
      notify(String(error), true);
    } finally {
      setExporting(false);
    }
  };
  const save = async (
    s: Server,
    secret = "",
    loginPathUpdate?: LoginPathUpdate,
  ) => {
    await api("save_server", {
      server: s,
      secret: secret || null,
      loginPathUpdate,
    });
    await refresh();
    notify("服务器已保存");
  };
  const probe = async (s: Server) => {
    setChecking(s.id);
    setMenu("");
    try {
      const fingerprint = await api<string>("probe_host", { id: s.id });
      setTrust({ server: s, fingerprint });
    } catch (e) {
      notify(String(e), true);
    } finally {
      setChecking("");
    }
  };
  const syncLoginPath = async (s: Server) => {
    setChecking(s.id);
    try {
      await api("sync_login_path", { id: s.id });
      await refresh();
      notify(s.name + " · PATH 已同步");
    } catch (error) {
      notify(String(error), true);
    } finally {
      setChecking("");
    }
  };
  const test = async (s: Server) => {
    if (!s.fingerprint) {
      void probe(s);
      return;
    }
    setChecking(s.id);
    try {
      await operate({ kind: "test", server_id: s.id });
      notify(s.name + " · 连接成功");
    } catch (e) {
      notify(String(e), true);
    } finally {
      setChecking("");
      void refresh();
    }
  };
  const visit = (s: Server, p: Page) => {
    if (p === "files") setFileServerId(s.id);
    setSelectedId(s.id);
    setPage(p);
    setMenu("");
  };
  const toggleMcp = async () => {
    setMcpBusy(true);
    try {
      if (data.mcp.running) await api("stop_mcp");
      else await api("start_mcp", { port, host: mcpHost });
      await refresh();
      notify(data.mcp.running ? "MCP 服务已停止" : "MCP 服务已启动");
    } catch (e) {
      notify(String(e), true);
    } finally {
      setMcpBusy(false);
    }
  };
  return (
    <div className="app" onClick={() => menu && setMenu("")}>
      <WindowBar
        title={titles[page]}
        actionsRef={setTitlebarActions}
        onError={(message) => notify(message, true)}
      >
        {page !== "terminals" && (
          <div className="page-actions">
            {page === "files" && (
              <div className="server-select">
                <ServerIcon size={15} />
                <SelectField
                  aria-label="选择服务器"
                  value={fileServer?.id || ""}
                  onValueChange={setFileServerId}
                >
                  <option value="">
                    {data.servers.length ? "请选择服务器" : "尚无服务器"}
                  </option>
                  {data.servers.map((s) => (
                    <option key={s.id} value={s.id} disabled={!s.enabled}>
                      {s.name}
                      {!s.enabled ? "（已停用）" : ""}
                    </option>
                  ))}
                </SelectField>
                <ChevronDown size={13} />
              </div>
            )}
          </div>
        )}
      </WindowBar>
      <aside className="sidebar">
        <div
          className="sidebar-brand"
          title={productName}
          data-tauri-drag-region
        >
          <img
            className="sidebar-app-icon"
            src={appIcon}
            alt=""
            draggable={false}
          />
          <span className="sidebar-brand-name" data-tauri-drag-region>
            <strong data-tauri-drag-region>Remote SSH</strong>
            <small data-tauri-drag-region>Brige</small>
          </span>
        </div>
        <nav aria-label="主导航">
          {navigation.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              title={label}
              aria-label={label}
              aria-current={page === id ? "page" : undefined}
              className={page === id ? "nav-item active" : "nav-item"}
              onClick={() => {
                if (id === "files" && page !== "files") setFileServerId("");
                setPage(id);
              }}
            >
              <Icon size={17} />
              <span>{label}</span>
              {id === "servers" && (
                <span className="nav-count">{data.servers.length}</span>
              )}
              {id === "terminals" && data.terminals.some((s) => s.alive) && (
                <span className="status-dot green" />
              )}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <ApprovalCenter
            onResolved={() => void refresh()}
            onOpenSettings={() => {
              setPage("settings");
              requestAnimationFrame(() =>
                document
                  .getElementById("security-settings")
                  ?.scrollIntoView({ block: "start" }),
              );
            }}
          />
          <button
            title="MCP 服务"
            aria-label="MCP 服务"
            aria-current={page === "mcp" ? "page" : undefined}
            className={"nav-item " + (page === "mcp" ? "active" : "")}
            onClick={() => setPage("mcp")}
          >
            <Plug size={17} />
            <span>MCP 服务</span>
            <span
              className={"status-dot " + (data.mcp.running ? "green" : "gray")}
            />
          </button>
          <button
            title="设置"
            aria-label="设置"
            aria-current={page === "settings" ? "page" : undefined}
            className={"nav-item " + (page === "settings" ? "active" : "")}
            onClick={() => setPage("settings")}
          >
            <Settings2 size={17} />
            <span>设置</span>
          </button>
        </div>
      </aside>
      <div className="main-shell">
        <main
          className={
            "page-content " +
            (page === "audit" || page === "jobs" || page === "tunnels"
              ? "audit-page"
              : page === "terminals"
                ? "terminal-page"
                : page === "settings"
                  ? "settings-page"
                  : page === "files"
                    ? "files-page"
                    : "") +
            (page === "audit" ? " audit-records-page" : "")
          }
        >
          {!desktop && (
            <div className="preview-banner">
              界面演示模式 · 真实连接请使用桌面应用
            </div>
          )}
          {loadError && (
            <div className="error-inline">加载失败：{loadError}</div>
          )}
          {page === "servers" && (
            <>
              <div className="server-toolbar">
                <div className="search-box">
                  <Search size={15} />
                  <Input
                    aria-label="搜索服务器"
                    placeholder="搜索名称、地址或账号"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                  />
                  {search && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="icon-btn"
                      aria-label="清空搜索"
                      onClick={() => setSearch("")}
                    >
                      <X size={13} />
                    </Button>
                  )}
                </div>
                <div className="filter-select">
                  <SelectField
                    aria-label="环境筛选"
                    value={group}
                    onValueChange={setGroup}
                  >
                    <option>全部服务器</option>
                    {groups.map((g) => (
                      <option key={g}>{g}</option>
                    ))}
                  </SelectField>
                  <ChevronDown size={13} />
                </div>
                <Button
                  variant="default"
                  size="sm"
                  className="btn primary server-add-button"
                  onClick={() => setModal(newServer())}
                >
                  <Plus size={15} />
                  添加服务器
                </Button>
              </div>
              <div className="server-list">
                {visible.map((s) => {
                  const Icon = serverIcon(s);
                  return (
                    <article
                      key={s.id}
                      className={
                        "server-card " + (!s.enabled ? "disabled-card" : "")
                      }
                    >
                      <div className={"server-avatar " + s.color}>
                        <Icon size={20} />
                      </div>
                      <div className="server-identity">
                        <div className="server-title">
                          <button onClick={() => setModal(s)}>{s.name}</button>
                          <Badge
                            variant="secondary"
                            className="env-tag font-normal"
                          >
                            {s.group}
                          </Badge>
                        </div>
                        <div className="server-address">
                          <span className="mono">
                            {s.username}@{s.host}:{s.port}
                          </span>
                          <span className="metadata-separator" />
                          <span>
                            {s.auth === "password"
                              ? "密码"
                              : s.auth === "agent"
                                ? "代理认证"
                                : "私钥"}
                          </span>
                        </div>
                      </div>
                      <div className="server-permission">
                        {mcpPermissionSummary(s)}
                        <div
                          className="text-xs text-muted-foreground"
                          title="仅同步 PATH；不继承 Conda 的其他变量、alias 或 shell 函数。远端环境变化后请重新同步。"
                        >
                          {s.login_path_synced_at ? (
                            <>
                              PATH ·{" "}
                              <time
                                dateTime={s.login_path_synced_at}
                                title={new Date(
                                  s.login_path_synced_at,
                                ).toLocaleString()}
                              >
                                {new Date(
                                  s.login_path_synced_at,
                                ).toLocaleString("zh-CN", {
                                  month: "2-digit",
                                  day: "2-digit",
                                  hour: "2-digit",
                                  minute: "2-digit",
                                  hour12: false,
                                })}
                              </time>
                            </>
                          ) : s.login_path != null ? (
                            "PATH · 手动设置"
                          ) : (
                            "PATH 未同步"
                          )}
                        </div>
                      </div>
                      <div className="server-buttons">
                        <Button
                          variant="ghost"
                          size="icon"
                          className="icon-btn"
                          disabled={!s.enabled}
                          title="浏览文件"
                          aria-label={"浏览文件 " + s.name}
                          onClick={() => visit(s, "files")}
                        >
                          <FolderOpen size={16} />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="icon-btn"
                          disabled={!s.enabled}
                          title="打开终端"
                          aria-label={"打开终端 " + s.name}
                          onClick={() => visit(s, "terminals")}
                        >
                          <TerminalSquare size={16} />
                        </Button>
                      </div>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="icon-btn"
                            title="更多操作"
                            aria-label={"更多操作 " + s.name}
                          >
                            <MoreHorizontal size={18} />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="min-w-36">
                          <DropdownMenuItem
                            disabled={!s.enabled || checking === s.id}
                            onSelect={() => void test(s)}
                          >
                            <Play size={14} />
                            测试连接
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            disabled={!s.enabled || checking === s.id}
                            onSelect={() => void syncLoginPath(s)}
                          >
                            <RefreshCw size={14} />
                            同步PATH
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem onSelect={() => setModal(s)}>
                            <Pencil size={14} />
                            编辑
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => void probe(s)}>
                            <ShieldCheck size={14} />
                            核验指纹
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            onSelect={() =>
                              void save({ ...s, enabled: !s.enabled }).catch(
                                (e) => notify(String(e), true),
                              )
                            }
                          >
                            <Power size={14} />
                            {s.enabled ? "停用" : "启用"}
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            className="text-destructive focus:text-destructive"
                            onSelect={() => setDeleting(s)}
                          >
                            <Trash2 size={14} />
                            删除
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </article>
                  );
                })}
                {visible.length === 0 && (
                  <div className="empty-panel">
                    <ServerIcon size={27} />
                    <h3>
                      {data.servers.length
                        ? "没有匹配的服务器"
                        : "尚未添加服务器"}
                    </h3>
                    {!data.servers.length && (
                      <Button
                        variant="outline"
                        size="sm"
                        className="btn"
                        onClick={() => setModal(newServer())}
                      >
                        <Plus size={14} />
                        添加服务器
                      </Button>
                    )}
                  </div>
                )}
              </div>
              {data.servers.length > 0 && (
                <div className="list-caption">
                  {visible.length} 台服务器 · {enabled.length} 台已启用
                </div>
              )}
            </>
          )}
          {page === "files" && (
            <Files
              key={fileServer?.id}
              server={fileServer}
              refresh={() => void refresh()}
              notify={notify}
            />
          )}
          {page === "terminals" && (
            <Suspense
              fallback={<div className="empty-panel">正在加载终端…</div>}
            >
              <Terminals
                headerHost={titlebarActions}
                servers={data.servers}
                onServerChange={setSelectedId}
                server={selected}
                sessions={data.terminals}
                refresh={refresh}
                notify={notify}
              />
            </Suspense>
          )}
          {page === "jobs" && <Jobs notify={notify} />}
          {page === "tunnels" && (
            <PortForwards servers={data.servers} notify={notify} />
          )}
          {page === "audit" && (
            <>
              <div className="audit-toolbar">
                <div className="audit-filter-summary">
                  <Tabs
                    className="audit-filters"
                    value={tab}
                    onValueChange={(category) => {
                      setTab(category);
                      setAuditQuery((query) => ({
                        ...query,
                        category,
                        offset: 0,
                        upper: undefined,
                      }));
                    }}
                  >
                    <TabsList aria-label="记录类型">
                      {[
                        ["all", "全部"],
                        ["files", "文件"],
                        ["exec", "命令"],
                        ["mcp", "MCP"],
                        ["error", "失败"],
                      ].map(([key, label]) => (
                        <TabsTrigger key={key} value={key}>
                          {label}
                        </TabsTrigger>
                      ))}
                    </TabsList>
                  </Tabs>
                  <span
                    className="audit-record-count"
                    role="status"
                    aria-label="记录条目数"
                  >
                    {auditPage.total.toLocaleString()} 条
                  </span>
                </div>
                <div className="audit-toolbar-controls">
                  <AuditTimeFilter
                    query={auditQuery}
                    onChange={(range) =>
                      setAuditQuery((query) => ({
                        ...query,
                        ...range,
                        offset: 0,
                        upper: undefined,
                      }))
                    }
                  />
                  <div className="audit-search search-box">
                    <Search size={14} />
                    <Input
                      placeholder="搜索操作记录"
                      aria-label="搜索操作记录"
                      value={auditSearch}
                      onChange={(e) => setAuditSearch(e.target.value)}
                    />
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    className="audit-export-button"
                    disabled={exporting || auditLoading || !!auditError}
                    aria-busy={exporting}
                    title="导出当前筛选条件下的全部记录"
                    onClick={() => void exportAudits()}
                  >
                    {exporting ? (
                      <LoaderCircle size={15} className="spinning" />
                    ) : (
                      <Download size={15} />
                    )}
                    <span>{exporting ? "导出中…" : "导出"}</span>
                  </Button>
                </div>
              </div>
              <div className="panel">
                <div
                  className="table-scroll"
                  ref={auditScroll}
                  onScroll={loadMoreAudits}
                  aria-label="操作记录列表"
                  aria-busy={auditLoading}
                >
                  <table className="data-table audit-table">
                    <colgroup>
                      <col className="audit-col-operation" />
                      <col className="audit-col-server" />
                      <col className="audit-col-source" />
                      <col className="audit-col-result" />
                      <col className="audit-col-duration" />
                      <col className="audit-col-time" />
                    </colgroup>
                    <thead>
                      <tr>
                        <th>操作</th>
                        <th className="audit-col-server">服务器</th>
                        <th className="audit-col-source">来源</th>
                        <th>结果</th>
                        <th className="audit-col-duration">耗时</th>
                        <th>时间</th>
                      </tr>
                    </thead>
                    <tbody>
                      {virtualAuditRows.length > 0 &&
                        virtualAuditRows[0].start > 0 && (
                          <tr aria-hidden="true" className="audit-spacer">
                            <td
                              colSpan={6}
                              style={{ height: virtualAuditRows[0].start }}
                            />
                          </tr>
                        )}
                      {virtualAuditRows.map((virtualRow) => {
                        const a = filteredAudits[virtualRow.index];
                        return (
                          <tr
                            key={a.id}
                            data-index={virtualRow.index}
                            ref={auditRows.measureElement}
                            tabIndex={0}
                            onClick={() => setAuditDetail(a)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") setAuditDetail(a);
                            }}
                          >
                            <td>
                              <strong>{labels[a.kind] || a.kind}</strong>
                              <p className="mono" title={a.detail}>
                                {a.detail || "—"}
                              </p>
                              <div className="audit-inline-context">
                                <span title={a.server_name}>
                                  {a.server_name}
                                </span>
                                <span>
                                  {a.source === "mcp" ? "MCP" : "手动操作"}
                                </span>
                              </div>
                            </td>
                            <td className="audit-col-server">
                              <span
                                className="audit-server-name"
                                title={a.server_name}
                              >
                                {a.server_name}
                              </span>
                              <span className="audit-server-source">
                                {a.source === "mcp" ? "MCP" : "手动操作"}
                              </span>
                            </td>
                            <td className="audit-col-source">
                              {a.source === "mcp" ? "MCP" : "手动操作"}
                            </td>
                            <td>
                              <span
                                className={
                                  "audit-status " +
                                  (auditSucceeded(a) ? "success" : "failed")
                                }
                              >
                                <span
                                  className={
                                    "status-dot " +
                                    (auditSucceeded(a) ? "green" : "red")
                                  }
                                />
                                {a.kind === "security_approve"
                                  ? "已批准"
                                  : a.kind === "security_reject"
                                    ? "已拒绝"
                                    : a.success
                                      ? "成功"
                                      : "失败"}
                              </span>
                              <span className="audit-inline-duration">
                                {a.duration_ms} 毫秒
                              </span>
                            </td>
                            <td className="mono muted audit-col-duration">
                              {a.duration_ms} 毫秒
                            </td>
                            <td className="muted">
                              <CompactTime value={a.time} />
                            </td>
                          </tr>
                        );
                      })}
                      {virtualAuditRows.length > 0 && (
                        <tr aria-hidden="true" className="audit-spacer">
                          <td
                            colSpan={6}
                            style={{
                              height: Math.max(
                                0,
                                auditRows.getTotalSize() -
                                  virtualAuditRows[virtualAuditRows.length - 1]
                                    .end,
                              ),
                            }}
                          />
                        </tr>
                      )}
                    </tbody>
                  </table>
                  {auditLoading ? (
                    <div className="audit-load-status" role="status">
                      正在加载操作记录…
                    </div>
                  ) : auditError ? (
                    <div className="empty-panel" role="alert">
                      {auditError}
                    </div>
                  ) : (
                    !filteredAudits.length && (
                      <div className="empty-panel">暂无记录</div>
                    )
                  )}
                  {!auditLoading &&
                    !auditError &&
                    filteredAudits.length > 0 && (
                      <div className="audit-load-status" role="status">
                        {filteredAudits.length >= auditPage.total
                          ? "已加载全部记录"
                          : `已加载 ${filteredAudits.length} 条，向下滚动加载更多`}
                      </div>
                    )}
                  {auditError && (
                    <div className="audit-load-status">
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => setAuditQuery((query) => ({ ...query }))}
                      >
                        重试加载
                      </Button>
                    </div>
                  )}
                </div>
              </div>
            </>
          )}
          {page === "mcp" && (
            <McpPanel
              status={data.mcp}
              servers={data.servers}
              port={port}
              setPort={setPort}
              host={mcpHost}
              setHost={setMcpHost}
              busy={mcpBusy}
              toggle={toggleMcp}
              edit={setModal}
              notify={notify}
            />
          )}
          {page === "settings" && <Settings />}
        </main>
      </div>
      {modal && (
        <ServerModal
          initial={modal}
          groups={groups}
          onOpenSettings={() => {
            setModal(null);
            setPage("settings");
            requestAnimationFrame(() =>
              document
                .getElementById("security-settings")
                ?.scrollIntoView({ block: "start" }),
            );
          }}
          onClose={() => setModal(null)}
          onSave={save}
          onPathSynced={() => void refresh()}
        />
      )}
      {trust && (
        <ModalFrame onClose={() => setTrust(null)} className="compact-modal">
          <div className="modal-layout">
            <div className="modal-head">
              <div className="heading-icon">
                <ShieldCheck size={22} />
              </div>
              <DialogTitle asChild>
                <h2>核验 SSH 主机指纹</h2>
              </DialogTitle>
            </div>
            <div className="modal-body">
              <p>
                请通过服务器控制台或管理员提供的信息核对以下指纹，然后信任此主机。
              </p>
              <div className="fingerprint-host">
                {trust.server.host}:{trust.server.port}
              </div>
              <pre className="fingerprint">{trust.fingerprint}</pre>
              {trust.server.fingerprint &&
                trust.server.fingerprint !== trust.fingerprint && (
                  <div className="error-inline">
                    主机指纹发生变化，请确认服务器是否更换了密钥。
                  </div>
                )}
              <p className="field-help">
                在服务器上运行 ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
                -E sha256
                可核验对应密钥。服务器协商了其他密钥类型时，请核验对应的公钥文件。
              </p>
            </div>
            <div className="modal-foot">
              <Button
                variant="outline"
                size="sm"
                className="btn"
                onClick={() => setTrust(null)}
              >
                取消
              </Button>
              <Button
                variant="default"
                size="sm"
                className="btn primary"
                onClick={async () => {
                  try {
                    await save({
                      ...trust.server,
                      fingerprint: trust.fingerprint,
                    });
                    setTrust(null);
                    notify("主机指纹已信任，可以测试连接");
                  } catch (e) {
                    notify(String(e), true);
                  }
                }}
              >
                <ShieldCheck size={15} />
                信任此指纹
              </Button>
            </div>
          </div>
        </ModalFrame>
      )}
      {deleting && (
        <ModalFrame onClose={() => setDeleting(null)} className="compact-modal">
          <div className="modal-layout">
            <div className="modal-head">
              <DialogTitle asChild>
                <h2>删除服务器？</h2>
              </DialogTitle>
            </div>
            <div className="modal-body">
              <p>
                将从本机移除「{deleting.name}
                」及其保存的凭据。远程服务器上的文件不会改变。请先关闭该服务器的终端。
              </p>
            </div>
            <div className="modal-foot">
              <Button
                variant="outline"
                size="sm"
                className="btn"
                onClick={() => setDeleting(null)}
              >
                取消
              </Button>
              <Button
                variant="destructive"
                size="sm"
                className="btn danger-fill"
                onClick={async () => {
                  try {
                    await api("remove_server", { id: deleting.id });
                    await refresh();
                    setDeleting(null);
                    notify("服务器已移除");
                  } catch (e) {
                    notify(String(e), true);
                  }
                }}
              >
                删除服务器
              </Button>
            </div>
          </div>
        </ModalFrame>
      )}
      {auditDetail && (
        <AuditDetails
          key={auditDetail.id}
          entry={auditDetail}
          onClose={() => setAuditDetail(null)}
          notify={notify}
        />
      )}
      {toast && (
        <div className={"toast " + (toast.error ? "error" : "")} role="status">
          {toast.error ? (
            <AlertTriangle size={18} />
          ) : (
            <CheckCircle2 size={18} />
          )}
          <span>{toast.message}</span>
          <Button
            variant="ghost"
            size="icon"
            className="icon-btn"
            aria-label="关闭提示"
            onClick={() => setToast(null)}
          >
            <X size={15} />
          </Button>
        </div>
      )}
    </div>
  );
}
