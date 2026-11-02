// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PortForwards } from "../components/PortForwards";
import { api } from "../api";
import { newServer } from "../types";
import type { TunnelInfo } from "../tunnels";

vi.mock("../api", () => ({ api: vi.fn(), desktop: true }));
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
});
afterEach(() => {
  cleanup();
  vi.resetAllMocks();
  localStorage.clear();
});
const server = {
  ...newServer(),
  id: "server-a",
  name: "测试主机",
  host: "10.0.0.1",
};
const item: TunnelInfo = {
  tunnel_id: "tunnel-a",
  server_id: server.id,
  server_name: server.name,
  source: "mcp",
  local_host: "127.0.0.1",
  local_port: 18080,
  remote_host: "127.0.0.1",
  remote_port: 8080,
  status: "running",
  created_at: "2026-11-01T12:00:00Z",
  connections: 2,
  bytes_sent: 2048,
  bytes_received: 4096,
  error: null,
};
function setup() {
  localStorage.setItem(
    "remote-desk.preferences.v1",
    JSON.stringify({ refreshSeconds: "0" }),
  );
  const notify = vi.fn();
  render(<PortForwards servers={[server]} refreshKey={0} notify={notify} />);
  return { user: userEvent.setup(), notify };
}
test("shows MCP-created mappings, traffic and close status", async () => {
  let closed = false;
  vi.mocked(api).mockImplementation(async (command, args) => {
    if (command === "port_forward") {
      closed = true;
      return {} as never;
    }
    return {
      tunnels: [{ ...item, status: closed ? "closed" : "running" }],
    } as never;
  });
  const { user } = setup();
  await screen.findByText("127.0.0.1:18080 → 127.0.0.1:8080");
  expect(screen.getByText("MCP")).toBeTruthy();
  expect(screen.getByText("发送 2.0 KiB / 接收 4.0 KiB")).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "关闭" }));
  await screen.findByText("已关闭");
  expect(api).toHaveBeenCalledWith("port_forward", {
    args: { action: "close", server_id: "server-a", tunnel_id: "tunnel-a" },
  });
});
test("creates an automatic loopback mapping and displays its assigned port", async () => {
  let created = false;
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "port_forward") {
      created = true;
      return item as never;
    }
    return { tunnels: created ? [item] : [] } as never;
  });
  const { user } = setup();
  await user.click(screen.getByRole("button", { name: "新建转发" }));
  await user.click(screen.getByRole("button", { name: "创建转发" }));
  await screen.findByText("127.0.0.1:18080 → 127.0.0.1:8080");
  expect(api).toHaveBeenCalledWith("port_forward", {
    args: {
      action: "open",
      server_id: "server-a",
      local_port: 0,
      remote_host: "127.0.0.1",
      remote_port: 8080,
    },
  });
});
test("keeps the form and reports permission or read-only failures", async () => {
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "port_forward") throw new Error("只读模式禁止创建端口转发");
    return { tunnels: [] } as never;
  });
  const { user } = setup();
  await user.click(screen.getByRole("button", { name: "新建转发" }));
  await user.click(screen.getByRole("button", { name: "创建转发" }));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain("只读模式"),
  );
  expect(screen.getByRole("dialog")).toBeTruthy();
});
