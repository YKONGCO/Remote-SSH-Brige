import { useEffect, useId, useRef, useState } from "react";
import { MAX_TRANSFER_MIB } from "../runtime-settings";
import { Input } from "./ui/input";
import { SelectField } from "./SelectField";

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
  const [unit, setUnit] = useState(1);
  const saving = useRef(false);
  const errorId = useId();
  useEffect(() => {
    setDraft(value === undefined ? "" : String(value / unit));
  }, [value, unit]);
  async function commit() {
    if (disabled || saving.current) return;
    const next = Number(draft) * unit;
    if (
      !/^\d+(?:\.\d+)?$/.test(draft) ||
      !Number.isSafeInteger(next) ||
      next < 0 ||
      next > MAX_TRANSFER_MIB
    ) {
      setError(`请输入有效大小，须对应整数 MiB；0 表示不限大小`);
      return;
    }
    setError("");
    if (next === value) return;
    saving.current = true;
    try {
      if (!(await onSave(next)))
        setDraft(value === undefined ? "" : String(value / unit));
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
          min={0}
          max={MAX_TRANSFER_MIB / unit}
          step={unit === 1 ? 1 : "any"}
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
              setDraft(value === undefined ? "" : String(value / unit));
              setError("");
            }
          }}
        />
        <SelectField
          className="transfer-limit-unit"
          aria-label={`${label}单位`}
          value={String(unit)}
          disabled={disabled}
          onValueChange={(v) => {
            setError("");
            setUnit(Number(v));
          }}
        >
          <option value="1">MiB</option>
          <option value="1024">GiB</option>
        </SelectField>
      </div>
      {error && (
        <p className="transfer-limit-error" id={errorId} role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
