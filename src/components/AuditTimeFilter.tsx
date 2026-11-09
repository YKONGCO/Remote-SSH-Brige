import { useLayoutEffect, useRef, useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { useCallback, type ReactNode } from "react";
import { CalendarDays, ChevronLeft, ChevronRight } from "lucide-react";
import type { AuditQuery } from "../types";
import { Button } from "./ui/button";

function dateKey(date: Date) {
  return `${String(date.getFullYear()).padStart(4, "0")}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function monthDate(year: number, month: number) {
  const date = new Date(0);
  date.setFullYear(year, month, 1);
  date.setHours(0, 0, 0, 0);
  return date;
}

function CalendarMonth({
  current,
  side,
  onChange,
  children,
}: {
  current: Date;
  side: string;
  onChange: (date: Date) => void;
  children: ReactNode;
}) {
  const [view, setView] = useState<"days" | "months" | "years">("days");
  const [yearPage, setYearPage] = useState(1);
  const year = current.getFullYear();
  const month = current.getMonth();
  const now = new Date();
  const maxYear = now.getFullYear();
  const maxMonth = now.getMonth();
  const navigate = (direction: number) => {
    if (view === "years")
      setYearPage((page) => Math.max(1, page + direction * 12));
    else
      onChange(
        monthDate(
          year + (view === "months" ? direction : 0),
          month + (view === "days" ? direction : 0),
        ),
      );
  };
  const atStart =
    view === "years"
      ? yearPage === 1
      : year === 1 && (view === "months" || month === 0);
  const atEnd =
    view === "years"
      ? yearPage + 12 > maxYear
      : year >= maxYear && (view === "months" || month >= maxMonth);
  const navigationLabels =
    view === "years"
      ? ["前12年", "后12年"]
      : view === "months"
        ? ["上一年", "下一年"]
        : ["上个月", "下个月"];

  return (
    <section
      className="audit-calendar-month"
      aria-label={`${side}日历：${year}年${month + 1}月`}
    >
      <div className="audit-calendar-month-header">
        <button
          type="button"
          className="audit-calendar-nav"
          aria-label={`${side}${navigationLabels[0]}`}
          disabled={atStart}
          onClick={() => navigate(-1)}
        >
          <ChevronLeft size={16} />
        </button>
        <div className="audit-calendar-heading">
          {view === "years" ? (
            <span>
              {yearPage}–{yearPage + 11}
            </span>
          ) : (
            <>
              <button
                type="button"
                aria-label={`${side}年份`}
                onClick={() => {
                  setYearPage(Math.floor((year - 1) / 12) * 12 + 1);
                  setView("years");
                }}
              >
                {year}年
              </button>
              <button
                type="button"
                aria-label={`${side}月份`}
                onClick={() => setView(view === "months" ? "days" : "months")}
              >
                {month + 1}月
              </button>
            </>
          )}
        </div>
        <button
          type="button"
          className="audit-calendar-nav"
          aria-label={`${side}${navigationLabels[1]}`}
          disabled={atEnd}
          onClick={() => navigate(1)}
        >
          <ChevronRight size={16} />
        </button>
      </div>
      {view === "days" ? (
        children
      ) : (
        <div className="audit-calendar-options">
          {Array.from({ length: 12 }, (_, index) => {
            const value = view === "years" ? yearPage + index : index;
            const label = `${view === "years" ? value : value + 1}${view === "years" ? "年" : "月"}`;
            const unavailable =
              view === "years"
                ? value > maxYear
                : year === maxYear && value > maxMonth;
            return (
              <button
                type="button"
                key={value}
                className="audit-calendar-option"
                aria-label={`${side}${label}`}
                aria-pressed={
                  view === "years" ? value === year : value === month
                }
                disabled={unavailable}
                onClick={() => {
                  onChange(
                    view === "years"
                      ? monthDate(
                          value,
                          value === maxYear ? Math.min(month, maxMonth) : month,
                        )
                      : monthDate(year, value),
                  );
                  setView(view === "years" ? "months" : "days");
                }}
              >
                {label}
              </button>
            );
          })}
        </div>
      )}
    </section>
  );
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
  disabled = false,
  placeholder = "时间范围",
  onOpenChange,
}: {
  query: AuditQuery;
  onChange: (range: Pick<AuditQuery, "start_time" | "end_time">) => void;
  disabled?: boolean;
  placeholder?: string;
  onOpenChange?: (open: boolean) => void;
}) {
  const appliedStart = localDate(query.start_time);
  const appliedEnd = localDate(query.end_time, true);
  const [open, setOpen] = useState(false);
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [hover, setHover] = useState("");
  const [months, setMonths] = useState(() => {
    const now = new Date();
    return [
      monthDate(now.getFullYear(), now.getMonth() - 1),
      monthDate(now.getFullYear(), now.getMonth()),
    ];
  });
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
  }, [open, months, place]);

  const changeOpen = (next: boolean) => {
    if (next) {
      setStart(appliedStart);
      setEnd(appliedEnd);
      setHover("");
      const initial = appliedStart
        ? new Date(`${appliedStart}T00:00:00`)
        : new Date();
      const now = new Date();
      const thisMonth = monthDate(now.getFullYear(), now.getMonth());
      let left = monthDate(
        initial.getFullYear(),
        initial.getMonth() - (appliedStart ? 0 : 1),
      );
      const last = appliedEnd ? new Date(`${appliedEnd}T00:00:00`) : null;
      let right =
        last &&
        (last.getFullYear() !== left.getFullYear() ||
          last.getMonth() !== left.getMonth())
          ? monthDate(last.getFullYear(), last.getMonth())
          : monthDate(left.getFullYear(), left.getMonth() + 1);
      if (right > thisMonth) {
        right = thisMonth;
        left = monthDate(now.getFullYear(), now.getMonth() - 1);
      }
      setMonths([left, right]);
    }
    setOpen(next);
    onOpenChange?.(next);
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
    onOpenChange?.(false);
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
          : placeholder;

  return (
    <Dialog.Root open={open} onOpenChange={changeOpen} modal={false}>
      <Dialog.Trigger asChild>
        <Button
          ref={trigger}
          variant="outline"
          size="sm"
          className="audit-time-trigger"
          disabled={disabled}
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
            {months.map((current, index) => {
              const year = current.getFullYear();
              const monthIndex = current.getMonth();
              const padding = (current.getDay() + 6) % 7;
              const nextMonth = monthDate(year, monthIndex + 1);
              nextMonth.setDate(0);
              const days = nextMonth.getDate();
              return (
                <CalendarMonth
                  key={index}
                  current={current}
                  side={index === 0 ? "左侧" : "右侧"}
                  onChange={(next) => {
                    setHover("");
                    setMonths((previous) =>
                      previous.map((month, i) => (i === index ? next : month)),
                    );
                  }}
                >
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
                </CalendarMonth>
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
                onOpenChange?.(false);
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
