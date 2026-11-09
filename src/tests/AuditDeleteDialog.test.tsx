// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { AuditDeleteDialog } from "../components/AuditDeleteDialog";
import { api } from "../api";

vi.mock("../api", () => ({ api: vi.fn() }));
beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-11-09T12:00:00"));
});
afterEach(() => {
  cleanup();
  vi.resetAllMocks();
  vi.useRealTimers();
});

test("calendar selection avoids native date segments and prevents deletion with incomplete or stale dates", async () => {
  const onDeleted = vi.fn();
  let finish: (value: unknown) => void = () => {};
  vi.mocked(api)
    .mockImplementationOnce(
      async () => ({ total: 12, upper: 42, entries: [] }) as never,
    )
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }) as never,
    );
  const rendered = render(
    <AuditDeleteDialog
      initialRange={{}}
      onClose={vi.fn()}
      onDeleted={onDeleted}
    />,
  );
  expect(rendered.container.querySelector('input[type="date"]')).toBeNull();
  expect(
    (
      screen.getByRole("button", {
        name: "删除",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  fireEvent.click(
    screen.getByRole("button", { name: "时间范围：选择日期范围" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "2026-11-02" }));
  expect(api).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "2026-11-01" }));
  await screen.findByText("12 条记录");
  expect(api).toHaveBeenCalledWith("audit_page", {
    query: expect.objectContaining({
      start_time: new Date("2026-11-01T00:00:00").toISOString(),
      end_time: new Date("2026-11-03T00:00:00").toISOString(),
    }),
  });
  fireEvent.click(
    screen.getByRole("button", { name: "时间范围：2026-11-01 至 2026-11-02" }),
  );
  expect(
    (
      screen.getByRole("button", {
        name: "删除",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "2026-11-01" }));
  fireEvent.click(screen.getByRole("button", { name: "2026-11-03" }));
  const pending = screen.getByRole("button", {
    name: "删除",
  }) as HTMLButtonElement;
  expect(pending.disabled).toBe(true);
  fireEvent.click(pending);
  await act(async () => {
    finish({ total: 0, upper: 42, entries: [] });
  });
  expect(
    (
      screen.getByRole("button", {
        name: "删除",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  expect(
    vi.mocked(api).mock.calls.every(([command]) => command === "audit_page"),
  ).toBe(true);
  expect(onDeleted).not.toHaveBeenCalled();
});
