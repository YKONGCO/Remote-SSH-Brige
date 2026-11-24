import { useEffect, useRef, useState } from "react";
import { ShieldCheck, X } from "lucide-react";
import { api, desktop } from "../api";
import type { ApprovalRequest } from "../types";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";

const ruleSourceLabels: Record<string, string> = {
  builtin: "内置规则",
  user: "自定义规则",
  legacy_whitelist: "旧版白名单",
  analysis: "静态分析",
};

export function ApprovalCenter({
  enabled,
  onResolved,
  onOpenSettings,
}: {
  enabled: boolean;
  onResolved: () => void;
  onOpenSettings?: () => void;
}) {
  const [requests, setRequests] = useState<ApprovalRequest[]>([]);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [now, setNow] = useState(Date.now);
  const known = useRef(new Set<string>());
  useEffect(() => {
    if (!desktop || !enabled) return;
    let disposed = false;
    let loading = false;
    const load = async () => {
      setNow(Date.now());
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
  }, [enabled]);
  if (!enabled) return null;
  const request = requests[0];
  const remainingSeconds = request
    ? Math.max(0, Math.ceil((Date.parse(request.expires_at) - now) / 1000))
    : 0;
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
        aria-label="命令审批"
        title={requests.length ? `${requests.length} 条命令待审批` : "命令审批"}
        onClick={() => setOpen(true)}
      >
        <ShieldCheck size={17} />
        <span>命令审批</span>
        {requests.length > 0 && (
          <span className="nav-count">{requests.length}</span>
        )}
      </button>
      {open && (
        <ModalFrame
          width={request ? 700 : 520}
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
              <h2>审批命令执行</h2>
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
                  <dd>MCP 客户端</dd>
                  <dt>风险</dt>
                  <dd>
                    {request.risk === "high"
                      ? "高（命中内置受保护规则）"
                      : request.risk === "protected"
                        ? "自定义受保护命令"
                        : "无法静态确认"}
                  </dd>
                  <dt>执行超时</dt>
                  <dd>
                    {request.timeout_secs > 0
                      ? `${request.timeout_secs} 秒`
                      : "应用默认"}
                  </dd>
                  <dt>有效期</dt>
                  <dd>
                    {new Date(request.expires_at).toLocaleTimeString("zh-CN", {
                      hour12: false,
                    })}{" "}
                    前
                    {remainingSeconds > 0
                      ? `（剩余 ${remainingSeconds} 秒，超时自动拒绝）`
                      : "（已超时，自动拒绝）"}
                  </dd>
                </dl>
                {request.reason && (
                  <p className="approval-warning">{request.reason}</p>
                )}
                <pre className="approval-code">{request.command}</pre>
                <div className="approval-rules">
                  <p>命中的规则</p>
                  <ul>
                    {request.matched_rules.map((rule, index) => (
                      <li key={`${rule.source}-${rule.command}-${index}`}>
                        <strong>
                          {ruleSourceLabels[rule.source] || rule.source}
                        </strong>
                        <code>
                          {rule.command}
                          {rule.argument ? ` ${rule.argument}` : ""}
                        </code>
                        <span>{rule.reason}</span>
                      </li>
                    ))}
                  </ul>
                </div>
                <p className="approval-digest">
                  命令 SHA-256：{request.command_sha256}
                  <br />
                  服务器配置 SHA-256：{request.server_sha256}
                </p>
                <p className="approval-warning">
                  批准仅对上述原始命令执行一次。执行前仍会重新校验服务器配置、权限、只读模式和命令策略；无法覆盖任何拒绝。
                </p>
              </>
            ) : (
              <div className="approval-empty">
                <ShieldCheck size={26} />
                <h3>暂无待审批命令</h3>
                <p>
                  当 Agent 通过 exec_approved
                  提交受保护命令时，会在这里等待你的单次批准。
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
                    打开安全设置
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
              <span>{requests.length} 条待处理</span>
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
                {busy ? "处理中…" : "批准本次执行"}
              </Button>
            </div>
          )}
        </ModalFrame>
      )}
    </>
  );
}
