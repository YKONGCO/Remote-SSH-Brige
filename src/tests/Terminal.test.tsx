// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { Terminals } from "../components/Terminal";
import { operate } from "../api";
import { newServer, type Session, type Operation } from "../types";

const { instances, MockTerminal } = vi.hoisted(() => {
  class MockTerminal {
    options: Record<string, unknown>;
    cols = 100;
    rows = 28;
    buffer = { active: { viewportY: 0, baseY: 10 } };
    listener?: (content: string) => void;
    write = vi.fn((_data: unknown, done?: () => void) => done?.());
    dispose = vi.fn();
    focus = vi.fn();
    scrollToLine = vi.fn();
    loadAddon = vi.fn();
    open = vi.fn();
    onData = vi.fn((listener: (content: string) => void) => {
      this.listener = listener;
      return { dispose: vi.fn() };
    });
    constructor(options: Record<string, unknown>) {
      this.options = options;
      instances.push(this);
    }
  }
  const instances: MockTerminal[] = [];
  return { instances, MockTerminal };
});
vi.mock("@xterm/xterm", () => ({ Terminal: MockTerminal }));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit = vi.fn();
  },
}));
vi.mock("../api", () => ({ desktop: true, operate: vi.fn() }));
beforeAll(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.resetAllMocks();
  instances.length = 0;
  localStorage.clear();
});
const a = { ...newServer(), id: "a", name: "服务器 A" };
const b = { ...newServer(), id: "b", name: "服务器 B" };
const sessions: Session[] = [
  {
    id: "a1",
    server_id: "a",
    name: "A first",
    alive: true,
    created_at: "2026-11-01T00:00:00Z",
  },
  {
    id: "a2",
    server_id: "a",
    name: "A second",
    alive: true,
    created_at: "2026-11-01T00:00:00Z",
  },
  {
    id: "b1",
    server_id: "b",
    name: "B first",
    alive: true,
    created_at: "2026-11-01T00:00:00Z",
  },
];
function stubOperations() {
  vi.mocked(operate).mockImplementation(async (op: Operation) => {
    if (op.kind === "terminal_read")
      return {
        stdout: op.cursor === 0 ? "session output" : "",
        cursor: 14,
        alive: true,
        truncated: false,
      } as never;
    return {} as never;
  });
}
function props() {
  return {
    server: a,
    servers: [a, b],
    sessions,
    onServerChange: vi.fn(),
    refresh: vi.fn().mockResolvedValue(undefined),
    notify: vi.fn(),
  };
}

test("switching tabs and servers retains terminal instances, scroll and read cursors", async () => {
  vi.useFakeTimers();
  stubOperations();
  const input = props();
  let view!: ReturnType<typeof render>;
  await act(async () => {
    view = render(<Terminals {...input} />);
  });
  expect(screen.queryByRole("button", { name: "刷新会话" })).toBeNull();
  const first = instances[0];
  first.buffer.active.viewportY = 4;
  await act(async () => {
    fireEvent.click(screen.getByRole("tab", { name: "终端 2" }));
  });
  const second = instances[1];
  await act(async () => {
    view.rerender(<Terminals {...input} server={b} />);
  });
  await act(async () => {
    view.rerender(<Terminals {...input} server={a} />);
  });
  expect(
    screen.getByRole("tab", { name: "终端 2" }).getAttribute("aria-selected"),
  ).toBe("true");
  await act(async () => {
    fireEvent.click(screen.getByRole("tab", { name: "终端 1" }));
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(2000);
  });
  expect(instances).toHaveLength(3);
  expect(first.dispose).not.toHaveBeenCalled();
  expect(second.dispose).not.toHaveBeenCalled();
  expect(first.buffer.active.viewportY).toBe(4);
  expect(
    vi
      .mocked(operate)
      .mock.calls.filter(
        ([op]) =>
          op.kind === "terminal_read" &&
          op.session_id === "a1" &&
          op.cursor === 0,
      ),
  ).toHaveLength(1);
  expect(operate).toHaveBeenCalledWith(
    expect.objectContaining({
      kind: "terminal_read",
      session_id: "a1",
      cursor: 14,
    }),
  );
  view.unmount();
  for (const terminal of instances)
    expect(terminal.dispose).toHaveBeenCalledOnce();
});

test("coalesces typing, splits Unicode pastes and never replays failed input", async () => {
  vi.useFakeTimers();
  stubOperations();
  const input = props();
  await act(async () => {
    render(<Terminals {...input} />);
  });
  const terminal = instances[0];
  act(() => {
    terminal.listener?.("l");
    terminal.listener?.("s");
    terminal.listener?.("\r");
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(30);
  });
  let writes = vi
    .mocked(operate)
    .mock.calls.filter(([op]) => op.kind === "terminal_write");
  expect(writes.map(([op]) => op.content)).toEqual(["ls\r"]);
  const paste = "中文😀".repeat(6000);
  act(() => {
    terminal.listener?.(paste);
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(100);
  });
  writes = vi
    .mocked(operate)
    .mock.calls.filter(([op]) => op.kind === "terminal_write")
    .slice(1);
  expect(writes.map(([op]) => op.content).join("")).toBe(paste);
  expect(writes.length).toBeGreaterThan(1);
  for (const [op] of writes)
    expect(new TextEncoder().encode(op.content).length).toBeLessThanOrEqual(
      16384,
    );
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "terminal_write") throw new Error("send failed");
    return { stdout: "", cursor: 14, alive: true, truncated: false } as never;
  });
  act(() => {
    terminal.listener?.("dangerous-command\r");
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(2000);
  });
  expect(
    vi
      .mocked(operate)
      .mock.calls.filter(
        ([op]) =>
          op.kind === "terminal_write" && op.content === "dangerous-command\r",
      ),
  ).toHaveLength(1);
  expect(input.notify).toHaveBeenCalledWith(
    expect.stringContaining("未自动重发"),
    true,
  );
});

test("read errors retry with the existing cursor and keep output available", async () => {
  vi.useFakeTimers();
  stubOperations();
  const input = props();
  await act(async () => {
    render(<Terminals {...input} />);
  });
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "terminal_read") throw new Error("temporary read failure");
    return {} as never;
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(screen.getByText("读取失败")).toBeTruthy();
  expect(instances[0].options.disableStdin).toBe(true);
  const count = vi.mocked(operate).mock.calls.length;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(vi.mocked(operate).mock.calls.length).toBe(count);
  stubOperations();
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "重试读取" }));
  });
  expect(screen.getByText("已连接")).toBeTruthy();
  expect(instances).toHaveLength(1);
  expect(
    vi
      .mocked(operate)
      .mock.calls.filter(
        ([op]) => op.kind === "terminal_read" && op.cursor === 0,
      ),
  ).toHaveLength(1);
});

test("ended sessions are removed directly and active closes prevent duplicate requests", async () => {
  vi.useFakeTimers();
  stubOperations();
  const input = {
    ...props(),
    sessions: [...sessions, { ...sessions[0], id: "ended", alive: false }],
  };
  await act(async () => {
    render(<Terminals {...input} />);
  });
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "移除终端 3" }));
  });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(operate).toHaveBeenCalledWith(
    expect.objectContaining({ kind: "terminal_close", session_id: "ended" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "关闭终端 1" }));
  const dialog = screen.getByRole("dialog");
  let finish!: (result: unknown) => void;
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "terminal_close")
      return new Promise((resolve) => {
        finish = resolve;
      }) as never;
    return { stdout: "", cursor: 14, alive: true, truncated: false } as never;
  });
  await act(async () => {
    fireEvent.click(within(dialog).getByRole("button", { name: "关闭会话" }));
  });
  const button = within(dialog).getByRole("button", {
    name: "正在关闭…",
  }) as HTMLButtonElement;
  expect(button.disabled).toBe(true);
  fireEvent.click(button);
  expect(
    vi
      .mocked(operate)
      .mock.calls.filter(
        ([op]) => op.kind === "terminal_close" && op.session_id === "a1",
      ),
  ).toHaveLength(1);
  await act(async () => {
    finish({ closed: true });
  });
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("remote EOF preserves final output, stops polling and blocks further input", async () => {
  vi.useFakeTimers();
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "terminal_read")
      return {
        stdout: "final output",
        cursor: 12,
        alive: false,
        truncated: false,
      } as never;
    if (op.kind === "terminal_open")
      return { session_id: "new-session" } as never;
    return {} as never;
  });
  const input = props();
  await act(async () => {
    render(<Terminals {...input} />);
  });
  const terminal = instances[0];
  expect(screen.getByText("连接已结束")).toBeTruthy();
  expect(terminal.options.disableStdin).toBe(true);
  expect(terminal.write.mock.calls[0][0]).toBe("final output");
  expect(typeof terminal.write.mock.calls[0][1]).toBe("function");
  act(() => {
    terminal.listener?.("do-not-send\r");
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(
    vi.mocked(operate).mock.calls.filter(([op]) => op.kind === "terminal_read"),
  ).toHaveLength(1);
  expect(
    vi.mocked(operate).mock.calls.some(([op]) => op.kind === "terminal_write"),
  ).toBe(false);
  expect(terminal.dispose).not.toHaveBeenCalled();
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "新建连接" }));
  });
  expect(operate).toHaveBeenCalledWith(
    expect.objectContaining({ kind: "terminal_open", server_id: "a" }),
  );
});

test("remote EOF drains remaining text pages before stopping", async () => {
  vi.useFakeTimers();
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind !== "terminal_read") return {} as never;
    return {
      stdout: op.cursor ? "末页" : "首页",
      cursor: op.cursor ? 12 : 6,
      alive: false,
      has_more: !op.cursor,
      truncated: false,
    } as never;
  });
  const input = props();
  input.sessions = [sessions[0]];
  await act(async () => {
    render(<Terminals {...input} />);
  });
  expect(instances[0].write.mock.calls[0][0]).toBe("首页");
  expect(instances[0].options.disableStdin).toBe(true);
  expect(screen.queryByText("连接已结束")).toBeNull();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(2000);
  });
  expect(instances[0].write.mock.calls.some(([text]) => text === "末页")).toBe(
    true,
  );
  expect(screen.getByText("连接已结束")).toBeTruthy();
  expect(
    vi.mocked(operate).mock.calls.filter(([op]) => op.kind === "terminal_read"),
  ).toHaveLength(2);
});
