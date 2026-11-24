import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ApprovalRequest } from "../types";
const mock = vi.hoisted(() => ({
  pending: [] as ApprovalRequest[],
  api: vi.fn(),
}));
vi.mock("../api", () => ({ desktop: true, api: mock.api }));
import { ApprovalCenter } from "../components/ApprovalCenter";
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});
function pending(
  expires = new Date(Date.now() + 120000).toISOString(),
): ApprovalRequest {
  return {
    id: "request-1",
    server_id: "server-1",
    server_name: "生产",
    host: "example.test:22",
    username: "root",
    source: "mcp",
    kind: "exec",
    command: 'cat "<img src=x onerror=alert(1)>"',
    reason: "Deletes files or directories; data may be unrecoverable",
    risk: "high",
    matched_rules: [
      {
        source: "builtin",
        command: "rm",
        reason: "Deletes files or directories; data may be unrecoverable",
      },
    ],
    command_bytes: 41,
    command_sha256: "a".repeat(64),
    server_sha256: "b".repeat(64),
    timeout_secs: 0,
    created_at: new Date().toISOString(),
    expires_at: expires,
  };
}
function setup(requests: ApprovalRequest[]) {
  mock.pending = requests;
  mock.api.mockImplementation(
    async (command: string, args?: { id: string; approved: boolean }) => {
      if (command === "pending_approvals") return [...mock.pending];
      if (command === "resolve_approval") {
        mock.pending = mock.pending.filter((item) => item.id !== args?.id);
        return;
      }
      throw new Error(command);
    },
  );
}
test("approval requires an explicit decision and renders remote commands as inert text", async () => {
  setup([pending()]);
  const user = userEvent.setup();
  const onResolved = vi.fn();
  render(<ApprovalCenter enabled onResolved={onResolved} />);
  await screen.findByRole("dialog", { name: "审批命令执行" });
  expect(screen.getByText(mock.pending[0].command)).toBeTruthy();
  expect(screen.getByText("高（命中内置受保护规则）")).toBeTruthy();
  expect(screen.getByText("内置规则")).toBeTruthy();
  expect(document.querySelector("img")).toBeNull();
  expect(
    mock.api.mock.calls.some(([command]) => command === "resolve_approval"),
  ).toBe(false);
  await user.click(screen.getByRole("button", { name: "拒绝" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(mock.api).toHaveBeenCalledWith("resolve_approval", {
    id: "request-1",
    approved: false,
  });
  expect(onResolved).toHaveBeenCalledOnce();
});
test("expired approvals cannot be accepted", async () => {
  setup([pending(new Date(Date.now() - 1000).toISOString())]);
  render(<ApprovalCenter enabled onResolved={vi.fn()} />);
  const approve = await screen.findByRole("button", {
    name: "批准本次执行",
  });
  expect((approve as HTMLButtonElement).disabled).toBe(true);
  expect(
    mock.api.mock.calls.some(([command]) => command === "resolve_approval"),
  ).toBe(false);
});

test("approval entry is hidden while approvals are unavailable", () => {
  setup([]);
  render(<ApprovalCenter enabled={false} onResolved={vi.fn()} />);
  expect(screen.queryByRole("button", { name: "命令审批" })).toBeNull();
});

test("approval entry with no requests links to protected command settings", async () => {
  setup([]);
  const user = userEvent.setup();
  const settings = vi.fn();
  render(
    <ApprovalCenter enabled onResolved={vi.fn()} onOpenSettings={settings} />,
  );
  await user.click(screen.getByRole("button", { name: "命令审批" }));
  await screen.findByText("暂无待审批命令");
  expect(screen.queryByRole("button", { name: "批准本次执行" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "打开安全设置" }));
  expect(settings).toHaveBeenCalledOnce();
  expect(screen.queryByRole("dialog")).toBeNull();
});
