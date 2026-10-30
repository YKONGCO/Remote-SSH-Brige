// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ApprovalRequest } from "../types";
const mock = vi.hoisted(() => ({
  pending: [] as ApprovalRequest[],
  api: vi.fn(),
}));
vi.mock("../api", () => ({ desktop: true, api: mock.api }));
import { ApprovalCenter } from "./ApprovalCenter";
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
    path: "",
    target: "",
    command: 'cat "<img src=x onerror=alert(1)>"',
    content_preview: "",
    content_bytes: 0,
    content_sha256: "",
    content_truncated: false,
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
  render(<ApprovalCenter onResolved={onResolved} />);
  await screen.findByRole("dialog", { name: "审批操作" });
  expect(screen.getByText(mock.pending[0].command)).toBeTruthy();
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
  render(<ApprovalCenter onResolved={vi.fn()} />);
  const approve = await screen.findByRole("button", { name: "批准本次" });
  expect((approve as HTMLButtonElement).disabled).toBe(true);
  expect(
    mock.api.mock.calls.some(([command]) => command === "resolve_approval"),
  ).toBe(false);
});
test("approval entry stays visible with no requests and links to approval settings", async () => {
  setup([]);
  const user = userEvent.setup();
  const settings = vi.fn();
  render(<ApprovalCenter onResolved={vi.fn()} onOpenSettings={settings} />);
  await user.click(screen.getByRole("button", { name: "操作审批" }));
  await screen.findByText("暂无待审批操作");
  expect(screen.queryByRole("button", { name: "批准本次" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "审批设置" }));
  expect(settings).toHaveBeenCalledOnce();
  expect(screen.queryByRole("dialog")).toBeNull();
});
