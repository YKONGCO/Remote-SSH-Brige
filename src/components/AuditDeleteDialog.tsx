import { useEffect, useState } from "react";
import { api } from "../api";
import type { AuditDeleteRange, AuditPage, AuditQuery } from "../types";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";
import { AuditTimeFilter } from "./AuditTimeFilter";

export function AuditDeleteDialog({
  initialRange,
  onClose,
  onDeleted,
}: {
  initialRange: Pick<AuditQuery, "start_time" | "end_time">;
  onClose: () => void;
  onDeleted: (count: number) => void;
}) {
  const [dates, setDates] = useState(initialRange);
  const start = dates.start_time;
  const end = dates.end_time;
  const [preview, setPreview] = useState<{
    key: string;
    total: number;
    range: AuditDeleteRange;
  } | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [picking, setPicking] = useState(false);
  const valid = !!start && !!end && Date.parse(start) < Date.parse(end);
  const key = `${start}/${end}`;
  const currentPreview = preview?.key === key ? preview : null;

  useEffect(() => {
    let active = true;
    setError("");
    setPreview(null);
    if (!valid) return;
    const range = { start_time: start!, end_time: end! };
    void api<AuditPage>("audit_page", {
      query: { ...range, category: "all", search: "", offset: 0, limit: 1 },
    })
      .then((result) => {
        if (active)
          setPreview({
            key,
            total: result.total,
            range: { ...range, upper: result.upper },
          });
      })
      .catch((e) => {
        if (active) setError(String(e));
      });
    return () => {
      active = false;
    };
  }, [start, end, key, valid]);

  async function remove() {
    if (!currentPreview?.total || busy || picking) return;
    setBusy(true);
    setError("");
    try {
      const count = await api<number>("delete_audits", {
        range: currentPreview.range,
      });
      onDeleted(count);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <ModalFrame
      className="audit-delete-modal"
      width={480}
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <div className="audit-delete-heading">
        <DialogTitle>删除操作记录</DialogTitle>
        <p>删除所选日期内的全部记录，无法恢复。</p>
      </div>
      <AuditTimeFilter
        query={{ ...dates, category: "all", search: "", offset: 0, limit: 1 }}
        onChange={setDates}
        placeholder="选择日期范围"
        disabled={busy}
        onOpenChange={setPicking}
      />
      {error && (
        <p className="error-inline" role="alert">
          {error}
        </p>
      )}
      <div className="audit-delete-footer">
        <span role="status" aria-live="polite">
          {currentPreview
            ? `${currentPreview.total.toLocaleString()} 条记录`
            : valid && !error
              ? "统计中…"
              : ""}
        </span>
        <div className="audit-delete-actions">
          <Button variant="outline" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button
            variant="destructive"
            disabled={busy || picking || !currentPreview?.total}
            onClick={() => void remove()}
          >
            {busy ? "删除中…" : "删除"}
          </Button>
        </div>
      </div>
    </ModalFrame>
  );
}
