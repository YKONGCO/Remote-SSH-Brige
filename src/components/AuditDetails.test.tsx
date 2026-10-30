// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import type { Audit } from "../types";
const mock = vi.hoisted(() => ({ api: vi.fn() }));
vi.mock("../api", () => ({ api: mock.api }));
import { AuditDetails } from "./AuditDetails";
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
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
