import { useEffect, useId, useRef, useState } from "react";
import { MAX_TRANSFER_MIB } from "../runtime-settings";
import { Input } from "./ui/input";

export function TransferLimitField({
  value,
  label,
  disabled,
  onSave,
}: {
  value?: number;
  label: string;
  disabled: boolean;
  onSave: (value: number) => Promise<boolean>;
}) {
  const [draft, setDraft] = useState(value === undefined ? "" : String(value));
  const [error, setError] = useState("");
  const saving = useRef(false);
  const errorId = useId();
  useEffect(() => {
    setDraft(value === undefined ? "" : String(value));
  }, [value]);
  async function commit() {
    if (disabled || saving.current) return;
    const next = Number(draft);
    if (
      !/^\d+$/.test(draft) ||
      !Number.isSafeInteger(next) ||
      next < 1 ||
      next > MAX_TRANSFER_MIB
    ) {
      setError(`请输入 1–${MAX_TRANSFER_MIB} 的整数`);
      return;
    }
    setError("");
    if (next === value) return;
    saving.current = true;
    try {
      if (!(await onSave(next)))
        setDraft(value === undefined ? "" : String(value));
    } finally {
      saving.current = false;
    }
  }
  return (
    <div className="transfer-limit-field">
      <div className="transfer-limit-control" data-invalid={Boolean(error)}>
        <Input
          type="number"
          inputMode="numeric"
          min={1}
          max={MAX_TRANSFER_MIB}
          step={1}
          aria-label={label}
          aria-invalid={Boolean(error)}
          aria-describedby={error ? errorId : undefined}
          disabled={disabled}
          value={draft}
          placeholder="加载中…"
          onChange={(event) => {
            setDraft(event.target.value);
            setError("");
          }}
          onBlur={() => void commit()}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              event.currentTarget.blur();
            }
            if (event.key === "Escape") {
              setDraft(value === undefined ? "" : String(value));
              setError("");
            }
          }}
        />
        <span>MiB</span>
      </div>
      {error && (
        <p className="transfer-limit-error" id={errorId} role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
