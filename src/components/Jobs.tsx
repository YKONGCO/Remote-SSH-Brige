import { useEffect, useState } from "react";
import { Eye, Search, X, RefreshCw } from "lucide-react";
import { api } from "../api";
import { usePreferences } from "../preferences";
import {
  type JobInfo,
  type JobOutput,
  type JobStatus,
  jobStatusLabels,
} from "../jobs";
import { CompactTime } from "./CompactTime";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";
import { SelectField } from "./SelectField";

function Status({ status, code }: { status: JobStatus; code: number | null }) {
  const color =
    status === "running"
      ? "green"
      : status === "disconnected" || (status === "exited" && code !== 0)
        ? "red"
        : "gray";
  return (
    <span className={`job-status ${color}`}>
      <span className={`status-dot ${color}`} />
      {jobStatusLabels[status]}
      {status === "exited" && code !== null ? ` · ${code}` : ""}
    </span>
  );
}
function duration(ms: number) {
  const seconds = Math.floor(ms / 1000);
  return seconds < 60
    ? `${seconds} 秒`
    : seconds < 3600
      ? `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`
      : `${Math.floor(seconds / 3600)} 小时 ${Math.floor((seconds % 3600) / 60)} 分`;
}

export function Jobs({
  refreshKey = 0,
  notify,
}: {
  refreshKey?: number;
  notify: (message: string, error?: boolean) => void;
}) {
  const preferences = usePreferences();
  const [items, setItems] = useState<JobInfo[]>([]);
  const [status, setStatus] = useState("running");
  const [search, setSearch] = useState("");
  const [server, setServer] = useState("all");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [revision, setRevision] = useState(0);
  const [selected, setSelected] = useState<JobInfo | null>(null);
  const [closing, setClosing] = useState<JobInfo | null>(null);
  const [closeError, setCloseError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let disposed = false,
      inFlight = false;
    setLoading(true);
    const load = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const result = await api<{ jobs: JobInfo[] }>("list_jobs");
        if (!disposed) {
          setItems(result.jobs);
          setError("");
        }
      } catch (e) {
        if (!disposed) setError(String(e));
      } finally {
        inFlight = false;
        if (!disposed) setLoading(false);
      }
    };
    void load();
    const timer =
      preferences.refreshSeconds === "0"
        ? undefined
        : setInterval(
            () => void load(),
            Number(preferences.refreshSeconds) * 1000,
          );
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [refreshKey, revision, preferences.refreshSeconds]);
  const servers = new Map(
    items.map((item) => [item.server_id, item.server_name || item.server_id]),
  );
  const visible = items.filter(
    (item) =>
      (status === "all" || item.status === status) &&
      (server === "all" || item.server_id === server) &&
      `${item.command} ${item.job_id} ${item.server_name} ${item.server_id}`
        .toLowerCase()
        .includes(search.trim().toLowerCase()),
  );
  const requestClose = (item: JobInfo) => {
    setSelected(null);
    setCloseError("");
    setClosing(item);
  };
  return (
    <>
      <div className="audit-toolbar jobs-toolbar">
        <div className="audit-filter-summary">
          <Tabs
            className="audit-filters"
            value={status}
            onValueChange={setStatus}
          >
            <TabsList aria-label="后台任务状态">
              {[
                ["running", "运行中"],
                ["all", "全部"],
                ["exited", "已退出"],
                ["closed", "已关闭"],
                ["disconnected", "已断线"],
              ].map(([key, label]) => (
                <TabsTrigger value={key} key={key}>
                  {label}
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          <span
            className="audit-record-count"
            role="status"
            aria-label="后台任务数量"
          >
            {visible.length} 个
          </span>
        </div>
        <div className="jobs-toolbar-controls">
          <SelectField
            aria-label="筛选后台任务服务器"
            className="jobs-server-filter"
            value={server}
            onValueChange={setServer}
          >
            <option value="all">全部服务器</option>
            {[...servers].map(([id, name]) => (
              <option key={id} value={id}>
                {name}
              </option>
            ))}
          </SelectField>
          <div className="audit-search search-box">
            <Search size={14} />
            <Input
              aria-label="搜索后台任务"
              placeholder="搜索命令、服务器或任务 ID"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
        </div>
      </div>
      <div className="panel">
        <div
          className="table-scroll jobs-table-scroll"
          aria-label="后台任务列表"
        >
          <table className="data-table audit-table jobs-table">
            <colgroup>
              <col className="job-col-command" />
              <col className="audit-col-server job-col-server" />
              <col className="audit-col-source job-col-source" />
              <col className="job-col-state" />
              <col className="audit-col-duration job-col-duration" />
              <col className="job-col-time" />
              <col className="job-col-actions" />
            </colgroup>
            <thead>
              <tr>
                <th>命令 / 后台任务</th>
                <th className="audit-col-server">服务器</th>
                <th className="audit-col-source">来源</th>
                <th>状态</th>
                <th className="audit-col-duration">持续时间</th>
                <th>创建时间</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((item) => (
                <tr
                  key={item.job_id}
                  tabIndex={0}
                  onClick={() => setSelected(item)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && e.target === e.currentTarget)
                      setSelected(item);
                  }}
                >
                  <td>
                    <strong className="mono" title={item.command}>
                      {item.command || "SSH 执行会话"}
                    </strong>
                    <p className="mono" title={item.job_id}>
                      {item.job_id}
                    </p>
                    <div className="audit-inline-context">
                      <span>{item.server_name || item.server_id}</span>
                      <span>{item.source === "mcp" ? "MCP" : "手动操作"}</span>
                    </div>
                  </td>
                  <td className="audit-col-server">
                    <span
                      className="audit-server-name"
                      title={item.server_name}
                    >
                      {item.server_name || item.server_id}
                    </span>
                  </td>
                  <td className="audit-col-source">
                    {item.source === "mcp" ? "MCP" : "手动操作"}
                  </td>
                  <td>
                    <Status status={item.status} code={item.exit_code} />
                    {item.error && (
                      <p className="job-error-preview" title={item.error}>
                        {item.error}
                      </p>
                    )}
                  </td>
                  <td className="audit-col-duration muted">
                    {duration(item.duration_ms)}
                  </td>
                  <td className="muted">
                    <CompactTime value={item.created_at} />
                  </td>
                  <td>
                    <div className="job-actions">
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label={`查看后台任务 ${item.job_id}`}
                        title="查看输出"
                        onClick={(e) => {
                          e.stopPropagation();
                          setSelected(item);
                        }}
                      >
                        <Eye size={15} />
                      </Button>
                      {item.status === "running" && (
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={`关闭后台任务 ${item.job_id}`}
                          title="关闭会话"
                          onClick={(e) => {
                            e.stopPropagation();
                            requestClose(item);
                          }}
                        >
                          <X size={15} />
                        </Button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {loading && (
            <div className="audit-load-status" role="status">
              正在加载后台任务…
            </div>
          )}
          {error && (
            <div className="empty-panel" role="alert">
              {error}
              <Button
                variant="outline"
                size="sm"
                onClick={() => setRevision((v) => v + 1)}
              >
                重试加载
              </Button>
            </div>
          )}
          {!loading && !error && !visible.length && (
            <div className="empty-panel">
              <strong>
                {items.length
                  ? "没有符合筛选条件的后台任务"
                  : "暂无保留的后台任务"}
              </strong>
              <p>命令超时并保留为后台任务后，会在这里显示。</p>
            </div>
          )}
          {!loading && !error && visible.length > 0 && (
            <div className="audit-load-status">
              已显示 {visible.length} 个后台任务 · 已结束会话保留 30 分钟
            </div>
          )}
        </div>
      </div>
      {selected && (
        <JobDetails
          job={selected}
          onClose={() => setSelected(null)}
          onCloseJob={() => requestClose(selected)}
        />
      )}
      {closing && (
        <ModalFrame
          className="compact-modal"
          onClose={() => {
            if (!busy) setClosing(null);
          }}
        >
          <form
            className="modal-layout"
            onSubmit={async (e) => {
              e.preventDefault();
              setBusy(true);
              setCloseError("");
              try {
                await api("job", {
                  serverId: closing.server_id,
                  jobId: closing.job_id,
                  action: "close",
                });
                setClosing(null);
                setRevision((v) => v + 1);
                notify("后台任务会话已关闭");
              } catch (e) {
                setCloseError(String(e));
                setRevision((v) => v + 1);
              } finally {
                setBusy(false);
              }
            }}
          >
            <div className="modal-head">
              <DialogTitle>关闭后台任务会话</DialogTitle>
            </div>
            <div className="modal-body">
              <pre className="audit-code">
                {closing.command || closing.job_id}
              </pre>
              <p>
                关闭将断开这次 SSH 执行连接，不发送
                kill，也不保证远端派生进程终止。
              </p>
              {closeError && (
                <p className="error-inline" role="alert">
                  {closeError}
                </p>
              )}
            </div>
            <div className="modal-foot flex gap-2 justify-end">
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => setClosing(null)}
              >
                取消
              </Button>
              <Button type="submit" disabled={busy}>
                {busy ? "关闭中…" : "关闭会话"}
              </Button>
            </div>
          </form>
        </ModalFrame>
      )}
    </>
  );
}

const OUTPUT_LIMIT = 256 * 1024;
function appendOutput(before: string, chunk: string) {
  const text = before + chunk;
  let start = Math.max(0, text.length - OUTPUT_LIMIT);
  if (
    start > 0 &&
    text.charCodeAt(start) >= 0xdc00 &&
    text.charCodeAt(start) <= 0xdfff
  )
    start++;
  return { text: text.slice(start), truncated: start > 0 };
}
function JobDetails({
  job,
  onClose,
  onCloseJob,
}: {
  job: JobInfo;
  onClose: () => void;
  onCloseJob: () => void;
}) {
  const [view, setView] = useState({
    stdout: "",
    stderr: "",
    status: job.status,
    code: job.exit_code,
    connectionError: job.error,
    truncated: false,
  });
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let out = 0,
      err = 0;
    setLoading(true);
    setError("");
    setView({
      stdout: "",
      stderr: "",
      status: job.status,
      code: job.exit_code,
      connectionError: job.error,
      truncated: false,
    });
    const read = async () => {
      try {
        const result = await api<JobOutput>("job", {
          serverId: job.server_id,
          jobId: job.job_id,
          action: "read",
          stdoutCursor: out,
          stderrCursor: err,
        });
        if (disposed) return;
        const advanced =
          result.next_stdout_cursor > out || result.next_stderr_cursor > err;
        out = result.next_stdout_cursor;
        err = result.next_stderr_cursor;
        setView((previous) => {
          const stdout = appendOutput(previous.stdout, result.stdout),
            stderr = appendOutput(previous.stderr, result.stderr);
          return {
            stdout: stdout.text,
            stderr: stderr.text,
            status: result.status,
            code: result.exit_code,
            connectionError: result.error,
            truncated:
              previous.truncated ||
              stdout.truncated ||
              stderr.truncated ||
              result.stdout_truncated ||
              result.stderr_truncated,
          };
        });
        setLoading(false);
        if (
          result.stdout_has_more ||
          result.stderr_has_more ||
          result.status === "running"
        )
          timer = setTimeout(
            () => void read(),
            advanced && (result.stdout_has_more || result.stderr_has_more)
              ? 0
              : 1000,
          );
      } catch (e) {
        if (!disposed) {
          setError(String(e));
          setLoading(false);
        }
      }
    };
    void read();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [job.job_id, job.server_id, retry]);
  return (
    <ModalFrame width={860} onClose={onClose}>
      <div className="modal-layout job-detail-layout">
        <div className="modal-head">
          <DialogTitle>后台任务输出</DialogTitle>
          <Button
            variant="ghost"
            size="icon"
            aria-label="关闭后台任务详情"
            onClick={onClose}
          >
            <X size={16} />
          </Button>
        </div>
        <div className="modal-body">
          <div className="audit-detail-meta">
            <span>{job.server_name || job.server_id}</span>
            <span>{job.source === "mcp" ? "MCP" : "手动操作"}</span>
            <Status status={view.status} code={view.code} />
            <CompactTime value={job.created_at} />
          </div>
          <pre className="audit-code">{job.command || job.job_id}</pre>
          <p className="mono muted">{job.job_id}</p>
          {error && (
            <p className="error-inline" role="alert">
              {error}
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setRetry((v) => v + 1)}
              >
                <RefreshCw size={13} />
                重新读取
              </Button>
            </p>
          )}
          {view.connectionError && (
            <p className="error-inline">{view.connectionError}</p>
          )}
          {view.truncated && (
            <p className="audit-message">
              较早输出已截断；界面每路最多显示最近 256 Ki 字符。
            </p>
          )}
          <section className="audit-section">
            <h3>标准输出</h3>
            <pre
              className="audit-code audit-output job-output"
              aria-label="后台任务标准输出"
              tabIndex={0}
            >
              {view.stdout || (loading ? "正在读取…" : "（无输出）")}
            </pre>
          </section>
          <section className="audit-section">
            <h3>错误输出</h3>
            <pre
              className="audit-code audit-output audit-stderr job-output"
              aria-label="后台任务错误输出"
              tabIndex={0}
            >
              {view.stderr || "（无输出）"}
            </pre>
          </section>
        </div>
        <div className="modal-foot flex gap-2 justify-end">
          {view.status === "running" && (
            <Button variant="outline" onClick={onCloseJob}>
              关闭会话
            </Button>
          )}
          <Button onClick={onClose}>关闭</Button>
        </div>
      </div>
    </ModalFrame>
  );
}
