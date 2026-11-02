import { useLayoutEffect, useRef, useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { useCallback } from "react";
import { CalendarDays, ChevronLeft, ChevronRight } from "lucide-react";
import type { AuditQuery } from "../types";
import { Button } from "./ui/button";

function dateKey(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function localDate(value?: string, previousDay = false) {
  if (!value) return "";
  const date = new Date(value);
  if (previousDay) date.setDate(date.getDate() - 1);
  return dateKey(date);
}

export function AuditTimeFilter({
  query,
  onChange,
}: {
  query: AuditQuery;
  onChange: (range: Pick<AuditQuery, "start_time" | "end_time">) => void;
}) {
  const appliedStart = localDate(query.start_time);
  const appliedEnd = localDate(query.end_time, true);
  const [open, setOpen] = useState(false);
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [hover, setHover] = useState("");
  const [month, setMonth] = useState(() => new Date());
  const [position, setPosition] = useState({ left: 0, top: 0 });
  const trigger = useRef<HTMLButtonElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const today = dateKey(new Date());

  const place = useCallback(() => {
    const anchor = trigger.current?.getBoundingClientRect();
    const popup = content.current?.getBoundingClientRect();
    if (!anchor || !popup) return;
    const left = Math.max(
      12,
      Math.min(anchor.left, window.innerWidth - popup.width - 12),
    );
    const below = anchor.bottom + 8;
    const top =
      below + popup.height <= window.innerHeight - 12
        ? below
        : Math.max(12, anchor.top - popup.height - 8);
    setPosition({ left, top });
  }, []);
  const attachContent = useCallback(
    (node: HTMLDivElement | null) => {
      content.current = node;
      if (node) place();
    },
    [place],
  );

  useLayoutEffect(() => {
    if (!open) return;
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, month, place]);

  const changeOpen = (next: boolean) => {
    if (next) {
      setStart(appliedStart);
      setEnd(appliedEnd);
      setHover("");
      const initial = appliedStart
        ? new Date(`${appliedStart}T00:00:00`)
        : new Date();
      setMonth(
        new Date(
          initial.getFullYear(),
          initial.getMonth() - (appliedStart ? 0 : 1),
          1,
        ),
      );
    }
    setOpen(next);
  };

  const select = (key: string) => {
    if (!start || end) {
      setStart(key);
      setEnd("");
      setHover("");
      return;
    }
    const first = key < start ? key : start;
    const last = key < start ? start : key;
    const exclusiveEnd = new Date(`${last}T00:00:00`);
    exclusiveEnd.setDate(exclusiveEnd.getDate() + 1);
    onChange({
      start_time: new Date(`${first}T00:00:00`).toISOString(),
      end_time: exclusiveEnd.toISOString(),
    });
    setOpen(false);
  };

  const previewEnd = end || hover || start;
  const rangeStart = start && previewEnd < start ? previewEnd : start;
  const rangeEnd = start && previewEnd < start ? start : previewEnd;
  const label =
    appliedStart && appliedEnd
      ? `${appliedStart} 至 ${appliedEnd}`
      : appliedStart
        ? `${appliedStart} 起`
        : appliedEnd
          ? `截至 ${appliedEnd}`
          : "时间范围";

  return (
    <Dialog.Root open={open} onOpenChange={changeOpen} modal={false}>
      <Dialog.Trigger asChild>
        <Button
          ref={trigger}
          variant="outline"
          size="sm"
          className="audit-time-trigger"
          aria-label={`时间范围：${label}`}
        >
          <CalendarDays size={15} />
          <span>{label}</span>
        </Button>
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Content
          ref={attachContent}
          className="audit-calendar"
          style={position}
          aria-describedby={undefined}
        >
          <Dialog.Title className="sr-only">选择操作记录日期范围</Dialog.Title>
          <div className="audit-calendar-months">
            <button
              type="button"
              className="audit-calendar-nav previous"
              aria-label="上个月"
              onClick={() =>
                setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))
              }
            >
              <ChevronLeft size={18} />
            </button>
            <button
              type="button"
              className="audit-calendar-nav next"
              aria-label="下个月"
              disabled={
                new Date(month.getFullYear(), month.getMonth() + 1, 1) >=
                new Date(new Date().getFullYear(), new Date().getMonth(), 1)
              }
              onClick={() =>
                setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))
              }
            >
              <ChevronRight size={18} />
            </button>
            {[0, 1].map((offset) => {
              const current = new Date(
                month.getFullYear(),
                month.getMonth() + offset,
                1,
              );
              const year = current.getFullYear();
              const monthIndex = current.getMonth();
              const padding = (current.getDay() + 6) % 7;
              const days = new Date(year, monthIndex + 1, 0).getDate();
              return (
                <section
                  className="audit-calendar-month"
                  key={`${year}-${monthIndex}`}
                  aria-label={`${year}年${monthIndex + 1}月`}
                >
                  <h3>
                    {year}年{monthIndex + 1}月
                  </h3>
                  <div className="audit-calendar-weekdays" aria-hidden="true">
                    {["一", "二", "三", "四", "五", "六", "日"].map((day) => (
                      <span key={day}>{day}</span>
                    ))}
                  </div>
                  <div
                    className="audit-calendar-days"
                    onMouseLeave={() => setHover("")}
                  >
                    {Array.from({ length: padding }, (_, index) => (
                      <span key={`blank-${index}`} />
                    ))}
                    {Array.from({ length: days }, (_, index) => {
                      const day = index + 1;
                      const key = dateKey(new Date(year, monthIndex, day));
                      const selected =
                        key === start ||
                        key === (end || (hover && start ? hover : start));
                      const inRange =
                        !!start && key >= rangeStart && key <= rangeEnd;
                      return (
                        <span
                          key={key}
                          className={`audit-calendar-cell${inRange ? " in-range" : ""}${key === rangeStart ? " range-start" : ""}${key === rangeEnd ? " range-end" : ""}`}
                        >
                          <button
                            type="button"
                            aria-label={key}
                            aria-pressed={selected}
                            aria-current={key === today ? "date" : undefined}
                            disabled={key > today}
                            className={`audit-calendar-day${selected ? " selected" : ""}`}
                            onClick={() => select(key)}
                            onMouseEnter={() => {
                              if (start && !end) setHover(key);
                            }}
                            onFocus={() => {
                              if (start && !end) setHover(key);
                            }}
                          >
                            {day}
                          </button>
                        </span>
                      );
                    })}
                  </div>
                </section>
              );
            })}
          </div>
          <div className="audit-calendar-footer">
            <span role="status">
              {start && !end ? "请选择结束日期" : "选择开始和结束日期"}
            </span>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                onChange({ start_time: undefined, end_time: undefined });
                setOpen(false);
              }}
            >
              清除
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
