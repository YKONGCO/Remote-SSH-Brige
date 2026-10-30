// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { McpPanel } from "./McpPanel";
import { api } from "../api";
import type { McpClient } from "../mcp-clients";
vi.mock("../api", () => ({ desktop: true, api: vi.fn() }));
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
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});
const client: McpClient = {
  id: "cursor",
  name: "Cursor",
  path: "C:/test/.cursor/mcp.json",
  status: "available",
};
const props = {
  status: { running: true, port: 18920, token: "fixture-token" },
  servers: [],
  port: 18920,
  setPort: vi.fn(),
  busy: false,
  toggle: vi.fn(),
  edit: vi.fn(),
  notify: vi.fn(),
};

test("refresh keeps client rows mounted while awaiting updated status", async () => {
  vi.mocked(api).mockResolvedValueOnce([client]);
  const view = render(<McpPanel {...props} />);
  const row = await screen.findByText("Cursor");
  let finish!: (value: McpClient[]) => void;
  vi.mocked(api).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve as typeof finish;
      }),
  );
  view.rerender(
    <McpPanel {...props} status={{ ...props.status, token: "new" }} />,
  );
  expect(screen.getByText("Cursor")).toBe(row);
  expect(screen.queryByText("正在检测客户端配置…")).toBeNull();
  await act(async () => finish([{ ...client, status: "outdated" }]));
  expect(screen.getByText("Cursor")).toBe(row);
});

test("wildcard binding requires a reachable IP for remote configuration", async () => {
  const user = userEvent.setup();
  vi.mocked(api).mockResolvedValue([client]);
  render(<McpPanel {...props} status={{ ...props.status, host: "0.0.0.0" }} />);
  expect(
    (screen.getByRole("button", { name: "复制配置" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await user.type(
    screen.getByRole("textbox", { name: "客户端连接 IP" }),
    "192.168.1.10",
  );
  const configuration = JSON.parse(
    screen.getByText(/"mcpServers"/).textContent!,
  );
  expect(configuration.mcpServers["remote-desk"].url).toBe(
    "http://192.168.1.10:18920/mcp",
  );
  expect(configuration.mcpServers["remote-desk"].headers.Authorization).toBe(
    "Bearer fixture-token",
  );
});
test("Pi exposes one-click configuration alongside the generic JSON config", async () => {
  const user = userEvent.setup();
  vi.mocked(api).mockResolvedValue([
    { ...client, id: "pi", name: "Pi", path: "C:/test/.pi/agent/mcp.json" },
  ]);
  render(<McpPanel {...props} />);
  await user.click(await screen.findByRole("button", { name: "一键配置" }));
  expect(api).toHaveBeenCalledWith("configure_mcp_client", { id: "pi" });
  expect(screen.queryByRole("combobox", { name: "配置格式" })).toBeNull();
  expect(screen.getByText(/"mcpServers"/).textContent).toContain(
    '"remote-desk"',
  );
});
test("one click configures the selected client and token changes reload status", async () => {
  const user = userEvent.setup();
  vi.mocked(api).mockImplementation(
    async (command) =>
      (command === "configure_mcp_client"
        ? [{ ...client, status: "configured" }]
        : [client]) as never,
  );
  const view = render(<McpPanel {...props} />);
  await user.click(await screen.findByRole("button", { name: "一键配置" }));
  expect(api).toHaveBeenCalledWith("configure_mcp_client", { id: "cursor" });
  await screen.findByRole("button", { name: "已配置" });
  expect(
    (screen.getByRole("button", { name: "已配置" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  view.rerender(
    <McpPanel {...props} status={{ ...props.status, token: "rotated" }} />,
  );
  await waitFor(() =>
    expect(
      vi
        .mocked(api)
        .mock.calls.filter(([command]) => command === "mcp_clients"),
    ).toHaveLength(2),
  );
});
test("manual JSON is always visible and copies the original schema", async () => {
  const user = userEvent.setup();
  vi.mocked(api).mockResolvedValue([client]);
  const view = render(<McpPanel {...props} />);
  await screen.findByText("Cursor");
  const configuration = screen.getByText(/"mcpServers"/).textContent!;
  expect(JSON.parse(configuration)).toEqual({
    mcpServers: {
      "remote-desk": {
        url: "http://127.0.0.1:18920/mcp",
        headers: { Authorization: "Bearer fixture-token" },
      },
    },
  });
  expect(screen.queryByRole("button", { name: "查看配置" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "复制配置" }));
  expect(await navigator.clipboard.readText()).toBe(configuration);
  view.rerender(
    <McpPanel {...props} status={{ ...props.status, running: false }} />,
  );
  expect(
    (
      screen.getByRole("button", {
        name: /复制配置|已复制/,
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "一键配置" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true),
  );
});
