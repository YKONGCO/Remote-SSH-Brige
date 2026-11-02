// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AuditTimeFilter } from "../components/AuditTimeFilter";

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-11-02T12:00:00"));
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});
const query = { category: "all", search: "", offset: 50, limit: 50 };

test("two-month calendar selects across dates, previews range and clears", () => {
  const onChange = vi.fn();
  const { rerender } = render(
    <AuditTimeFilter query={query} onChange={onChange} />,
  );
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /时间范围/ }));
  expect(screen.getByRole("region", { name: "2026年10月" })).toBeTruthy();
  expect(screen.getByRole("region", { name: "2026年11月" })).toBeTruthy();
  expect(
    (screen.getByRole("button", { name: "2026-11-03" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "2026-10-27" }));
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.mouseEnter(screen.getByRole("button", { name: "2026-11-02" }));
  expect(
    screen
      .getByRole("button", { name: "2026-10-30" })
      .parentElement?.classList.contains("in-range"),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "2026-11-02" }));
  const range = {
    start_time: new Date("2026-10-27T00:00:00").toISOString(),
    end_time: new Date("2026-11-03T00:00:00").toISOString(),
  };
  expect(onChange).toHaveBeenLastCalledWith(range);
  expect(screen.queryByRole("dialog")).toBeNull();
  rerender(
    <AuditTimeFilter query={{ ...query, ...range }} onChange={onChange} />,
  );
  fireEvent.click(
    screen.getByRole("button", { name: /时间范围：2026-10-27 至 2026-11-02/ }),
  );
  expect(
    screen
      .getByRole("button", { name: "2026-10-27" })
      .getAttribute("aria-pressed"),
  ).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "清除" }));
  expect(onChange).toHaveBeenLastCalledWith({
    start_time: undefined,
    end_time: undefined,
  });
});

test("supports reverse selection, same-day selection, month navigation and Escape cancellation", () => {
  const onChange = vi.fn();
  render(<AuditTimeFilter query={query} onChange={onChange} />);
  fireEvent.click(screen.getByRole("button", { name: /时间范围/ }));
  fireEvent.click(screen.getByRole("button", { name: "2026-11-02" }));
  fireEvent.click(screen.getByRole("button", { name: "2026-10-31" }));
  expect(onChange).toHaveBeenLastCalledWith({
    start_time: new Date("2026-10-31T00:00:00").toISOString(),
    end_time: new Date("2026-11-03T00:00:00").toISOString(),
  });
  fireEvent.click(screen.getByRole("button", { name: /时间范围/ }));
  fireEvent.click(screen.getByRole("button", { name: "2026-10-31" }));
  fireEvent.click(screen.getByRole("button", { name: "2026-10-31" }));
  expect(onChange).toHaveBeenLastCalledWith({
    start_time: new Date("2026-10-31T00:00:00").toISOString(),
    end_time: new Date("2026-11-01T00:00:00").toISOString(),
  });
  fireEvent.click(screen.getByRole("button", { name: /时间范围/ }));
  fireEvent.click(screen.getByRole("button", { name: "上个月" }));
  expect(screen.getByRole("region", { name: "2026年9月" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "下个月" }));
  fireEvent.click(screen.getByRole("button", { name: "2026-10-01" }));
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(onChange).toHaveBeenCalledTimes(2);
});
