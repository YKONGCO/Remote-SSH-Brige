import { useEffect, useState } from "react";
import { Copy, RefreshCw, X } from "lucide-react";
import { api } from "../api";
import { labels, type Audit } from "../types";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";

export function AuditDetails({
  entry,
  onClose,
  notify,
}: {
  entry: Audit;
  onClose: () => void;
  notify: (message: string, error?: boolean) => void;
}) {
  const [detail, setDetail] = useState(entry),
    [loading, setLoading] = useState(true),
    [error, setError] = useState("");
  useEffect(() => {
    let stale = false;
    setDetail(entry);
    setLoading(true);
    setError("");
    void api<Audit>("audit_details", { id: entry.id })
      .then((result) => {
        if (!stale) setDetail(result);
      })
      .catch((e) => {
        if (!stale) setError(String(e));
      })
      .finally(() => {
        if (!stale) setLoading(false);
      });
    return () => {
      stale = true;
    };
  }, [entry.id]);
  const response = detail.response;
  const data = response?.data;
  // New records use message; legacy records may have a string error.
  const failureMessage =
    typeof data?.code === "string" && typeof data?.message === "string"
      ? data.message
      : typeof data?.error === "string"
        ? data.error
        : null;
  const approval =
    data && typeof data.approval === "object" && data.approval !== null
      ? (data.approval as Record<string, unknown>)
      : detail.kind.startsWith("security_") && data?.request_id
        ? data
        : null;
  const approvalResult = approval
    ? approval.status === "approved" || approval.approved === true
      ? "已批准"
      : "已拒绝"
    : null;
  const date = (value: unknown) =>
    typeof value === "string" && Number.isFinite(Date.parse(value))
      ? new Date(value).toLocaleString("zh-CN", { hour12: false })
      : "—";
  const approvalFields: [string, string][] = approval
    ? [
        ["审批结果", approvalResult || "—"],
        [
          "请求操作",
          labels[String(approval.kind)] || String(approval.kind || "—"),
        ],
        [
          "发起来源",
          approval.request_source === "mcp"
            ? "MCP 客户端"
            : approval.request_source === "desktop"
              ? "桌面操作"
              : "—",
        ],
        ["审批人", approval.decision_by === "desktop" ? "本机用户" : "—"],
        ["服务器地址", String(approval.host || "—")],
        ["SSH 账户", String(approval.username || "—")],
        ["请求编号", String(approval.request_id || "—")],
        ["申请时间", date(approval.requested_at)],
        ["处理时间", date(approval.decided_at)],
        ["有效期至", date(approval.expires_at)],
        ...(approval.kind === "remove_path"
          ? [
              [
                "删除范围",
                approval.recursive
                  ? "整个目录及其内容（不跟随符号链接）"
                  : "仅文件、符号链接或空目录",
              ] as [string, string],
              [
                "目标不存在",
                approval.missing_ok !== false ? "视为已完成" : "返回错误",
              ] as [string, string],
            ]
          : []),
        ...(approval.target
          ? [["目标路径", String(approval.target)] as [string, string]]
          : []),
        ...(typeof approval.content_bytes === "number" &&
        approval.content_bytes > 0
          ? [
              [
                "内容大小",
                `${approval.content_bytes.toLocaleString()} 字节${approval.kind === "upload" ? "（Base64）" : ""}`,
              ] as [string, string],
              ["SHA-256", String(approval.content_sha256 || "—")] as [
                string,
                string,
              ],
            ]
          : []),
      ]
    : [];
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(
        JSON.stringify(
          {
            input: response?.input ?? null,
            operation: detail.detail,
            response,
          },
          null,
          2,
        ),
      );
      notify("已复制");
    } catch (e) {
      notify(String(e), true);
    }
  };
  return (
    <ModalFrame onClose={onClose} className="audit-modal" width={620}>
      <div className="modal-layout">
        <div className="modal-head">
          <DialogTitle asChild>
            <h2>操作详情</h2>
          </DialogTitle>
          <span
            className={
              "audit-result " +
              ((approvalResult ? approvalResult === "已批准" : detail.success)
                ? "success"
                : "failed")
            }
          >
            {approvalResult ||
              (detail.kind === "security_approve"
                ? "已批准"
                : detail.kind === "security_reject"
                  ? "已拒绝"
                  : detail.success
                    ? "成功"
                    : "失败")}
          </span>
          <Button
            variant="ghost"
            size="icon"
            className="icon-btn push-right"
            aria-label="关闭"
            onClick={onClose}
          >
            <X size={16} />
          </Button>
        </div>
        <div className="modal-body audit-detail">
          <div className="audit-meta">
            <span>{labels[detail.kind] || detail.kind}</span>
            <span>{detail.server_name}</span>
            <span>{detail.source === "mcp" ? "MCP" : "手动操作"}</span>
            <span>{detail.duration_ms} 毫秒</span>
            <time>
              {new Date(detail.time).toLocaleString("zh-CN", { hour12: false })}
            </time>
          </div>
          {(!response?.input || approval) && (
            <section className="audit-section">
              <h3>
                {detail.kind === "exec" || approval?.command
                  ? "命令"
                  : "操作目标"}
              </h3>
              <pre className="audit-code">
                {String(
                  approval?.command || approval?.path || detail.detail || "—",
                )}
              </pre>
            </section>
          )}
          <section className="audit-section">
            <h3>输入参数</h3>
            {response?.input ? (
              <>
                <pre className="audit-code audit-output" tabIndex={0}>
                  {JSON.stringify(response.input.data, null, 2)}
                </pre>
                {response.input.truncated && (
                  <p className="audit-message">输入参数较长，仅保留预览。</p>
                )}
              </>
            ) : (
              <p className="audit-message">
                {loading
                  ? "正在读取…"
                  : error
                    ? "输入参数加载失败，请重试。"
                    : "这条记录未保存输入参数。"}
              </p>
            )}
          </section>
          <section className="audit-section">
            <div className="audit-section-heading">
              <h3>{approval ? "审批详情" : "返回结果"}</h3>
              {data && typeof data.exit_code === "number" && (
                <span>退出码 {data.exit_code}</span>
              )}
              <Button
                variant="ghost"
                size="icon"
                className="icon-btn"
                title="复制详情"
                aria-label="复制详情"
                disabled={loading || !response}
                onClick={() => void copy()}
              >
                <Copy size={14} />
              </Button>
            </div>
            {loading ? (
              <div className="audit-message">
                <RefreshCw size={13} className="spinning" />
                正在读取…
              </div>
            ) : error ? (
              <div className="error-inline">
                {error}
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setLoading(true);
                    setError("");
                    void api<Audit>("audit_details", { id: entry.id })
                      .then(setDetail)
                      .catch((e) => setError(String(e)))
                      .finally(() => setLoading(false));
                  }}
                >
                  重试
                </Button>
              </div>
            ) : !response ? (
              <div className="audit-message">这条旧记录未保存返回内容。</div>
            ) : approval ? (
              <dl className="audit-approval-meta">
                {approvalFields.map(([label, value]) => (
                  <div key={label}>
                    <dt>{label}</dt>
                    <dd>{value}</dd>
                  </div>
                ))}
              </dl>
            ) : (detail.kind === "exec" ||
                detail.kind === "job" ||
                detail.kind === "terminal_read") &&
              data &&
              typeof data.stdout === "string" ? (
              <>
                {typeof data.job_id === "string" && (
                  <p className="audit-message">
                    后台任务：{data.job_id} · {String(data.status || "—")}
                  </p>
                )}
                {failureMessage && (
                  <p className="error-inline">{failureMessage}</p>
                )}
                <div className="audit-output-label">标准输出</div>
                <pre className="audit-code audit-output" tabIndex={0}>
                  {String(data.stdout || "（无输出）")}
                </pre>
                {Boolean(data.stderr) && (
                  <>
                    <div className="audit-output-label danger">错误输出</div>
                    <pre
                      className="audit-code audit-output audit-stderr"
                      tabIndex={0}
                    >
                      {String(data.stderr)}
                    </pre>
                  </>
                )}
              </>
            ) : (
              <pre
                className={
                  "audit-code audit-output " +
                  (failureMessage ? "audit-stderr" : "")
                }
                tabIndex={0}
              >
                {failureMessage && !data?.code
                  ? failureMessage
                  : JSON.stringify(data, null, 2)}
              </pre>
            )}
            {response?.truncated && (
              <p className="audit-message">返回内容较长，仅保留预览。</p>
            )}
          </section>
        </div>
      </div>
    </ModalFrame>
  );
}
