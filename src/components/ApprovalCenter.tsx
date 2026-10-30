import { useEffect, useRef, useState } from "react";
import { ShieldCheck, X } from "lucide-react";
import { api, desktop } from "../api";
import { labels, type ApprovalRequest } from "../types";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";

export function ApprovalCenter({
  onResolved,
  onOpenSettings,
}: {
  onResolved: () => void;
  onOpenSettings?: () => void;
}) {
  const [requests, setRequests] = useState<ApprovalRequest[]>([]);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const known = useRef(new Set<string>());
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let loading = false;
    const load = async () => {
      if (loading) return;
      loading = true;
      try {
        const next = await api<ApprovalRequest[]>("pending_approvals");
        if (disposed) return;
        if (next.some((request) => !known.current.has(request.id)))
          setOpen(true);
        if (known.current.size > 0 && next.length === 0) setOpen(false);
        known.current = new Set(next.map((request) => request.id));
        setRequests(next);
      } catch {
        if (!disposed) setError("无法刷新审批请求，请检查桌面服务。");
      } finally {
        loading = false;
      }
    };
    void load();
    const timer = setInterval(() => void load(), 1000);
    window.addEventListener("focus", load);
    return () => {
      disposed = true;
      clearInterval(timer);
      window.removeEventListener("focus", load);
    };
  }, []);
  const request = requests[0];
  async function decide(approved: boolean) {
    if (!request || busy) return;
    setBusy(true);
    setError("");
    try {
      await api("resolve_approval", { id: request.id, approved });
      setRequests((current) =>
        current.filter((item) => item.id !== request.id),
      );
      if (requests.length === 1) setOpen(false);
      onResolved();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <button
        className="nav-item approval-entry"
        aria-label="操作审批"
        title={requests.length ? `${requests.length} 项操作待审批` : "操作审批"}
        onClick={() => setOpen(true)}
      >
        <ShieldCheck size={17} />
        <span>操作审批</span>
        {requests.length > 0 && (
          <span className="nav-count">{requests.length}</span>
        )}
      </button>
      {open && (
        <ModalFrame
          width={request ? 660 : 520}
          className={
            request ? "approval-modal" : "approval-modal approval-empty-modal"
          }
          onClose={() => {
            if (!busy) setOpen(false);
          }}
        >
          <div className="modal-head">
            <div className="heading-icon">
              <ShieldCheck size={18} />
            </div>
            <DialogTitle asChild>
              <h2>审批操作</h2>
            </DialogTitle>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="icon-btn"
              aria-label="关闭审批窗口"
              disabled={busy}
              onClick={() => setOpen(false)}
            >
              <X size={16} />
            </Button>
          </div>
          <div className="modal-body approval-body">
            {request ? (
              <>
                <dl className="approval-meta">
                  <dt>服务器</dt>
                  <dd>
                    {request.server_name} · {request.username}@{request.host}
                  </dd>
                  <dt>来源</dt>
                  <dd>
                    {request.source === "mcp" ? "MCP 客户端" : "桌面操作"}
                  </dd>
                  <dt>操作</dt>
                  <dd>{labels[request.kind] || request.kind}</dd>
                  {request.kind === "remove_path" && (
                    <>
                      <dt>删除范围</dt>
                      <dd>
                        {request.recursive
                          ? "整个目录及其内容（不跟随符号链接）"
                          : "仅文件、符号链接或空目录"}
                      </dd>
                      <dt>目标不存在</dt>
                      <dd>
                        {request.missing_ok !== false
                          ? "视为已完成"
                          : "返回错误"}
                      </dd>
                    </>
                  )}
                  <dt>有效期</dt>
                  <dd>
                    {new Date(request.expires_at).toLocaleTimeString("zh-CN", {
                      hour12: false,
                    })}{" "}
                    前
                  </dd>
                </dl>
                {request.path && (
                  <pre className="approval-code">
                    {request.path}
                    {request.target ? `\n→ ${request.target}` : ""}
                  </pre>
                )}
                {request.command && (
                  <pre className="approval-code">{request.command}</pre>
                )}
                {request.content_bytes > 0 && (
                  <div className="approval-payload">
                    <p>
                      请求内容：{request.content_bytes.toLocaleString()} 字节
                      {request.kind === "upload" ? "（Base64）" : ""}
                    </p>
                    {request.content_preview && (
                      <pre className="approval-code">
                        {request.content_preview}
                      </pre>
                    )}
                    {request.content_truncated && (
                      <p className="approval-warning">
                        内容较长，当前仅显示前 8,192 个字符。
                      </p>
                    )}
                    <p className="approval-digest">
                      SHA-256：{request.content_sha256}
                    </p>
                  </div>
                )}
                <p className="approval-warning">
                  批准仅对本次请求生效；安全保护和服务器权限仍会再次校验。
                </p>
              </>
            ) : (
              <div className="approval-empty">
                <ShieldCheck size={26} />
                <h3>暂无待审批操作</h3>
                <p>
                  {desktop
                    ? "开启审批模式后，敏感操作会在这里等待处理。"
                    : "审批请求需要在桌面应用中处理。"}
                </p>
                {onOpenSettings && (
                  <Button
                    variant="outline"
                    className="btn"
                    onClick={() => {
                      setOpen(false);
                      onOpenSettings();
                    }}
                  >
                    审批设置
                  </Button>
                )}
              </div>
            )}
            {error && (
              <p className="error-inline" role="alert">
                {error}
              </p>
            )}
          </div>
          {request && (
            <div className="modal-foot">
              <span>{requests.length} 项待处理</span>
              <Button
                variant="outline"
                className="btn"
                disabled={busy}
                onClick={() => void decide(false)}
              >
                拒绝
              </Button>
              <Button
                className="btn primary"
                disabled={busy || Date.parse(request.expires_at) <= Date.now()}
                onClick={() => void decide(true)}
              >
                {busy ? "处理中…" : "批准本次"}
              </Button>
            </div>
          )}
        </ModalFrame>
      )}
    </>
  );
}
