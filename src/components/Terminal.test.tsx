// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { newServer } from "../types";
import { defaults, savePreferences } from "../preferences";
const operations = vi.hoisted(() => ({
  call: vi.fn(),
  terminals: [] as Record<string, unknown>[],
}));
vi.mock("../api", () => ({ desktop: true, operate: operations.call }));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    options: Record<string, unknown>;
    constructor(options: Record<string, unknown>) {
      this.options = options;
      operations.terminals.push(options);
    }
    cols = 100;
    rows = 30;
    loadAddon() {}
    open() {}
    write() {}
    focus() {}
    dispose() {}
    onData() {
      return { dispose() {} };
    }
  },
}));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));
import { Terminals } from "./Terminal";
beforeAll(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  Element.prototype.scrollIntoView = vi.fn();
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
});
beforeEach(() => {
  operations.terminals.length = 0;
  operations.call.mockReset();
  operations.call.mockImplementation(async (op: { kind: string }) =>
    op.kind === "terminal_read"
      ? { base64: "", cursor: 0, alive: true, truncated: false }
      : op.kind === "terminal_open"
        ? { session_id: "new-session" }
        : {},
  );
});

test("terminal resolves shared theme tokens live without recreating its session", async () => {
  localStorage.clear();
  savePreferences({ ...defaults, theme: "light", terminalTheme: "app" });
  document.documentElement.dataset.theme = "light";
  document.documentElement.style.setProperty(
    "--terminal-background",
    "#f9f9f9",
  );
  const view = render(
    <Terminals
      server={server}
      servers={[server]}
      onServerChange={vi.fn()}
      sessions={[
        {
          id: "theme-session",
          server_id: server.id,
          name: "theme",
          created_at: new Date().toISOString(),
          alive: true,
        },
      ]}
      refresh={vi.fn().mockResolvedValue(undefined)}
      notify={vi.fn()}
    />,
  );
  await waitFor(() =>
    expect(operations.terminals[0].theme).toMatchObject({
      background: "#f9f9f9",
    }),
  );
  act(() => {
    document.documentElement.dataset.theme = "dark";
    document.documentElement.style.setProperty(
      "--terminal-background",
      "#333333",
    );
    savePreferences({ ...defaults, theme: "dark", terminalTheme: "app" });
  });
  await waitFor(() =>
    expect(operations.terminals[0].theme).toMatchObject({
      background: "#333333",
    }),
  );
  expect(operations.terminals).toHaveLength(1);
  expect(operations.terminals[0].fontWeight).toBe(360);
  expect(
    operations.call.mock.calls.some(([op]) => op.kind === "terminal_close"),
  ).toBe(false);
  view.unmount();
  localStorage.clear();
  document.documentElement.style.removeProperty("--terminal-background");
  delete document.documentElement.dataset.theme;
});
test("terminal defaults to dark independently of the app and can switch to light", async () => {
  localStorage.clear();
  document.documentElement.dataset.theme = "light";
  const view = render(
    <Terminals
      server={server}
      servers={[server]}
      onServerChange={vi.fn()}
      sessions={[
        {
          id: "independent-theme",
          server_id: server.id,
          name: "theme",
          created_at: new Date().toISOString(),
          alive: true,
        },
      ]}
      refresh={vi.fn().mockResolvedValue(undefined)}
      notify={vi.fn()}
    />,
  );
  await waitFor(() =>
    expect(
      document
        .querySelector(".terminal-window")
        ?.getAttribute("data-terminal-theme"),
    ).toBe("dark"),
  );
  expect(operations.terminals[0].fontWeight).toBe(360);
  act(() => savePreferences({ ...defaults, terminalTheme: "light" }));
  await waitFor(() =>
    expect(
      document
        .querySelector(".terminal-window")
        ?.getAttribute("data-terminal-theme"),
    ).toBe("light"),
  );
  expect(operations.terminals[0].fontWeight).toBe(400);
  expect(operations.terminals).toHaveLength(1);
  view.unmount();
  localStorage.clear();
  delete document.documentElement.dataset.theme;
});
afterEach(cleanup);
const server = {
  ...newServer(),
  id: "server-one",
  name: "测试服务器",
  auth: "agent" as const,
};
const second = { ...server, id: "server-two", name: "另一台服务器" };
test("terminal header has one create action and tabs close the intended session without switching the active tab", async () => {
  const user = userEvent.setup();
  const refresh = vi.fn().mockResolvedValue(undefined);
  const notify = vi.fn();
  render(
    <Terminals
      server={server}
      servers={[server, second]}
      onServerChange={vi.fn()}
      sessions={["one", "two"].map((id) => ({
        id,
        server_id: server.id,
        name: server.name,
        created_at: new Date().toISOString(),
        alive: true,
      }))}
      refresh={refresh}
      notify={notify}
    />,
  );
  expect(screen.getByRole("heading", { name: "终端会话" })).toBeTruthy();
  expect(screen.getAllByRole("button", { name: "新建终端" })).toHaveLength(1);
  const secondTab = screen.getByRole("tab", { name: "终端 2" });
  await user.click(secondTab);
  expect(secondTab.getAttribute("aria-selected")).toBe("true");
  await waitFor(() =>
    expect(operations.call).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "terminal_read", session_id: "two" }),
    ),
  );
  await user.click(screen.getByRole("button", { name: "关闭终端 1" }));
  expect(secondTab.getAttribute("aria-selected")).toBe("true");
  await user.click(screen.getByRole("button", { name: "关闭会话" }));
  await waitFor(() =>
    expect(operations.call).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "terminal_close", session_id: "one" }),
    ),
  );
  await user.click(screen.getByRole("button", { name: "新建终端" }));
  await waitFor(() =>
    expect(operations.call).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "terminal_open", server_id: server.id }),
    ),
  );
  expect(notify).not.toHaveBeenCalled();
});
test("server selection is available in the terminal header and does not close existing sessions", async () => {
  const user = userEvent.setup();
  const change = vi.fn();
  render(
    <Terminals
      server={server}
      servers={[server, second]}
      onServerChange={change}
      sessions={[]}
      refresh={vi.fn().mockResolvedValue(undefined)}
      notify={vi.fn()}
    />,
  );
  await user.click(screen.getByRole("combobox", { name: "选择服务器" }));
  await user.click(await screen.findByRole("option", { name: second.name }));
  expect(change).toHaveBeenCalledWith(second.id);
  expect(operations.call).not.toHaveBeenCalledWith(
    expect.objectContaining({ kind: "terminal_close" }),
  );
  expect(screen.getAllByRole("button", { name: "新建终端" })).toHaveLength(1);
});
