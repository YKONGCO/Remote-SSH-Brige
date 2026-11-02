// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import type { Audit } from "../types";
const mock = vi.hoisted(() => ({ api: vi.fn() }));
vi.mock("../api", () => ({ api: mock.api }));
import { AuditDetails } from "../components/AuditDetails";
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

test.each([
  {
    code: "CONNECTION_LOST",
    message: "连接已断开",
    details: { causes: ["reset by peer"] },
  },
  { error: "连接已断开" },
])(
  "audit errors support current and legacy records without repeating messages",
  async (data) => {
    const entry: Audit = {
      id: "error-audit",
      time: "2026-11-01T12:00:00Z",
      server_id: "s",
      server_name: "测试服务器",
      source: "mcp",
      kind: "exec",
      detail: "echo hello",
      success: false,
      duration_ms: 10,
      summary: "连接已断开",
    };
    mock.api.mockResolvedValue({
      ...entry,
      response: { data, truncated: false },
    });
    render(<AuditDetails entry={entry} onClose={vi.fn()} notify={vi.fn()} />);
    const result = await screen.findByText(/连接已断开/);
    expect(result.textContent?.match(/连接已断开/g)?.length).toBe(1);
    if ("code" in data) expect(result.textContent).toContain("reset by peer");
  },
);

test("audit details display recorded input parameters and command output", async () => {
  const entry: Audit = {
    id: "exec-audit",
    time: "2026-11-01T12:00:00Z",
    server_id: "s",
    server_name: "测试服务器",
    source: "mcp",
    kind: "exec",
    detail: "echo hello",
    success: true,
    duration_ms: 10,
    summary: "完成",
  };
  mock.api.mockResolvedValue({
    ...entry,
    response: {
      input: {
        data: {
          tool: "exec",
          arguments: {
            server_id: "s",
            command: "echo hello",
            timeout_secs: 30,
          },
        },
        truncated: true,
      },
      data: { stdout: "hello from SSH", stderr: "", exit_code: 0 },
      truncated: false,
    },
  });
  render(<AuditDetails entry={entry} onClose={vi.fn()} notify={vi.fn()} />);
  expect(await screen.findByText(/"timeout_secs": 30/)).toBeTruthy();
  expect(screen.getByText("hello from SSH")).toBeTruthy();
  expect(screen.getByText("输入参数较长，仅保留预览。")).toBeTruthy();
});

test("legacy audit details explicitly indicate missing input parameters", async () => {
  const entry: Audit = {
    id: "old",
    time: "2026-11-01T12:00:00Z",
    server_id: "s",
    server_name: "测试服务器",
    source: "mcp",
    kind: "read_file",
    detail: "/tmp/test",
    success: true,
    duration_ms: 10,
    summary: "完成",
  };
  mock.api.mockResolvedValue({
    ...entry,
    response: { data: { bytes: 123 }, truncated: false },
  });
  render(<AuditDetails entry={entry} onClose={vi.fn()} notify={vi.fn()} />);
  expect(await screen.findByText("这条记录未保存输入参数。")).toBeTruthy();
  expect(screen.getByText(/"bytes": 123/)).toBeTruthy();
});
test("approval audits show readable decision metadata and the complete original command", async () => {
  const command = `ls /tmp/${"long-name-".repeat(160)}`;
  const entry: Audit = {
    id: "audit-1",
    time: "2026-10-30T12:00:00Z",
    server_id: "server-1",
    server_name: "生产服务器",
    source: "mcp",
    kind: "security_reject",
    detail: command.slice(0, 1000),
    success: true,
    duration_ms: 0,
    summary: "审批已拒绝",
  };
  mock.api.mockResolvedValue({
    ...entry,
    response: {
      truncated: false,
      data: {
        approval: {
          request_id: "request-approval-1",
          status: "rejected",
          approved: false,
          kind: "exec",
          request_source: "mcp",
          decision_by: "desktop",
          host: "example.test:22",
          username: "admin",
          command,
          requested_at: "2026-10-30T11:59:30Z",
          decided_at: "2026-10-30T12:00:00Z",
          expires_at: "2026-10-30T12:01:30Z",
          content_bytes: 0,
        },
      },
    },
  });
  render(<AuditDetails entry={entry} onClose={vi.fn()} notify={vi.fn()} />);
  await screen.findByRole("heading", { name: "审批详情" });
  expect(screen.getByText(command)).toBeTruthy();
  expect(screen.getByText("request-approval-1")).toBeTruthy();
  expect(screen.getByText("MCP 客户端")).toBeTruthy();
  expect(screen.getByText("本机用户")).toBeTruthy();
  expect(screen.getByText("example.test:22")).toBeTruthy();
  expect(document.querySelector(".audit-result")?.textContent).toBe("已拒绝");
  expect(
    document.querySelector(".audit-result")?.classList.contains("failed"),
  ).toBe(true);
});
