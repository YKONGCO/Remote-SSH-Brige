// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
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
function setup(servers = [server]) {
  localStorage.setItem(
    "remote-desk.preferences.v1",
    JSON.stringify({ refreshSeconds: "0" }),
  );
  const notify = vi.fn();
  render(<PortForwards servers={servers} refreshKey={0} notify={notify} />);
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
  await screen.findByText("暂无运行中的端口转发");
  await user.click(screen.getByRole("tab", { name: "已关闭" }));
  expect(
    within(screen.getByLabelText("端口转发列表")).getByText("已关闭"),
  ).toBeTruthy();
  expect(api).toHaveBeenCalledWith("port_forward", {
    args: { action: "close", server_id: "server-a", tunnel_id: "tunnel-a" },
  });
});

test("filters retained history and deletes only the confirmed closed mapping, preserving failures for retry", async () => {
  let deleted = false;
  let fail = true;
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "delete_port_forward") {
      if (fail) throw new Error("history locked");
      deleted = true;
      return undefined as never;
    }
    return {
      tunnels: [
        item,
        ...(deleted
          ? []
          : [
              {
                ...item,
                tunnel_id: "closed-a",
                local_port: 19090,
                status: "closed",
              },
            ]),
      ],
    } as never;
  });
  const { user } = setup();
  await screen.findByText(item.tunnel_id);
  expect(screen.queryByRole("button", { name: /删除端口转发/ })).toBeNull();
  await user.click(screen.getByRole("tab", { name: "已关闭" }));
  expect(screen.queryByText(item.tunnel_id)).toBeNull();
  await user.type(
    screen.getByRole("textbox", { name: "搜索端口转发" }),
    "19090",
  );
  await user.click(
    screen.getByRole("button", { name: "删除端口转发 closed-a" }),
  );
  await user.click(screen.getByRole("button", { name: "取消" }));
  expect(api).not.toHaveBeenCalledWith(
    "delete_port_forward",
    expect.anything(),
  );
  await user.click(
    screen.getByRole("button", { name: "删除端口转发 closed-a" }),
  );
  await user.click(screen.getByRole("button", { name: "删除记录" }));
  expect(screen.getByRole("dialog")).toBeTruthy();
  fail = false;
  await user.click(screen.getByRole("button", { name: "删除记录" }));
  await waitFor(() => expect(screen.queryByText("closed-a")).toBeNull());
  expect(api).toHaveBeenCalledWith("delete_port_forward", { id: "closed-a" });
  await user.clear(screen.getByRole("textbox", { name: "搜索端口转发" }));
  await user.click(screen.getByRole("tab", { name: "全部" }));
  expect(screen.getByText(item.tunnel_id)).toBeTruthy();
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
  await user.click(screen.getByRole("tab", { name: "已关闭" }));
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

test("reopens a closed IPv6 mapping with its original ports and retains history", async () => {
  const history = {
    ...item,
    tunnel_id: "closed-ipv6",
    status: "closed",
    remote_host: "::1",
  };
  let reopened = false;
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "port_forward") {
      reopened = true;
      return { ...item, tunnel_id: "reopened-ipv6" } as never;
    }
    return {
      tunnels: [
        history,
        ...(reopened
          ? [{ ...item, tunnel_id: "reopened-ipv6", remote_host: "::1" }]
          : []),
      ],
    } as never;
  });
  const { user, notify } = setup();
  await user.click(screen.getByRole("tab", { name: "已关闭" }));
  await user.type(
    screen.getByRole("textbox", { name: "搜索端口转发" }),
    history.tunnel_id,
  );
  await user.click(
    screen.getByRole("button", {
      name: `启用端口转发 ${history.tunnel_id}`,
    }),
  );
  await screen.findByText("reopened-ipv6");
  expect(api).toHaveBeenCalledWith("port_forward", {
    args: {
      action: "open",
      server_id: server.id,
      local_port: 18080,
      remote_host: "::1",
      remote_port: 8080,
    },
  });
  expect(notify).toHaveBeenCalledWith("端口转发已重新启用");
  expect(
    screen.getByRole("textbox", { name: "搜索端口转发" }).getAttribute("value"),
  ).toBe("");
  await user.click(screen.getByRole("tab", { name: "已关闭" }));
  expect(screen.getByText(history.tunnel_id)).toBeTruthy();
  expect(api).not.toHaveBeenCalledWith(
    "delete_port_forward",
    expect.anything(),
  );
});

test("prevents duplicate reopen requests and keeps failed mappings available for retry", async () => {
  let rejectOpen!: (error: Error) => void;
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "port_forward")
      return new Promise((_, reject) => {
        rejectOpen = reject;
      });
    return { tunnels: [{ ...item, status: "closed" }] } as never;
  });
  const { user, notify } = setup();
  await user.click(screen.getByRole("tab", { name: "已关闭" }));
  const reopen = await screen.findByRole("button", {
    name: `启用端口转发 ${item.tunnel_id}`,
  });
  await user.dblClick(reopen);
  expect((reopen as HTMLButtonElement).disabled).toBe(true);
  expect(
    (
      screen.getByRole("button", {
        name: `删除端口转发 ${item.tunnel_id}`,
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  expect(
    vi.mocked(api).mock.calls.filter(([command]) => command === "port_forward"),
  ).toHaveLength(1);
  rejectOpen(new Error("本地端口已被占用"));
  await waitFor(() =>
    expect(notify).toHaveBeenCalledWith("Error: 本地端口已被占用", true),
  );
  expect(screen.getByText(item.tunnel_id)).toBeTruthy();
  expect((reopen as HTMLButtonElement).disabled).toBe(false);
  await user.click(reopen);
  expect(
    vi.mocked(api).mock.calls.filter(([command]) => command === "port_forward"),
  ).toHaveLength(2);
  rejectOpen(new Error("只读模式禁止创建端口转发"));
  await waitFor(() =>
    expect(notify).toHaveBeenCalledWith(
      "Error: 只读模式禁止创建端口转发",
      true,
    ),
  );
});

test.each([
  { reason: "deleted", servers: [] },
  { reason: "disabled", servers: [{ ...server, enabled: false }] },
])(
  "disables reopen when the original server is $reason",
  async ({ servers }) => {
    vi.mocked(api).mockResolvedValue({
      tunnels: [{ ...item, status: "closed" }],
    });
    const { user } = setup(servers);
    await user.click(screen.getByRole("tab", { name: "已关闭" }));
    const reopen = await screen.findByRole("button", {
      name: `启用端口转发 ${item.tunnel_id}`,
    });
    expect((reopen as HTMLButtonElement).disabled).toBe(true);
    await user.click(reopen);
    expect(api).not.toHaveBeenCalledWith("port_forward", expect.anything());
  },
);

test("finishes a pending close while periodic refresh is disabled", async () => {
  let requested = false;
  let closingReads = 0;
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "port_forward") {
      requested = true;
      return {} as never;
    }
    return {
      tunnels: [
        {
          ...item,
          status: !requested
            ? "running"
            : ++closingReads < 3
              ? "closing"
              : "closed",
        },
      ],
    } as never;
  });
  const { user } = setup();
  await screen.findByText(item.tunnel_id);
  await user.click(screen.getByRole("button", { name: "关闭" }));
  await screen.findByText("暂无运行中的端口转发", {}, { timeout: 3000 });
  await user.click(screen.getByRole("tab", { name: "已关闭" }));
  expect(screen.getByText(item.tunnel_id)).toBeTruthy();
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
