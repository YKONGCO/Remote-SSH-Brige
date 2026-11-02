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
import { Jobs } from "../components/Jobs";
import { api } from "../api";
import type { JobInfo } from "../jobs";

vi.mock("../api", () => ({ api: vi.fn() }));
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
const running: JobInfo = {
  job_id: "job_running",
  server_id: "server-a",
  server_name: "生产服务器",
  command: "tail -f /var/log/app.log",
  source: "mcp",
  created_at: "2026-11-01T10:00:00Z",
  status: "running",
  exit_code: null,
  error: null,
  duration_ms: 32000,
  stdout_end_cursor: 0,
  stderr_end_cursor: 0,
};
const exited: JobInfo = {
  ...running,
  job_id: "job_exited",
  server_id: "server-b",
  server_name: "开发服务器",
  command: "sleep 40 && echo completed",
  status: "exited",
  exit_code: 7,
};
function noPolling() {
  localStorage.setItem(
    "remote-desk.preferences.v1",
    JSON.stringify({ refreshSeconds: "0" }),
  );
}

test("后台任务 list starts with running sessions and filters retained history by status and search", async () => {
  noPolling();
  vi.mocked(api).mockResolvedValue({ jobs: [running, exited] });
  const user = userEvent.setup();
  render(<Jobs refreshKey={0} notify={vi.fn()} />);
  await screen.findByText(running.command);
  expect(screen.queryByText(exited.command)).toBeNull();
  await user.click(screen.getByRole("tab", { name: "全部" }));
  expect(screen.getByText(exited.command)).toBeTruthy();
  await user.type(
    screen.getByRole("textbox", { name: "搜索后台任务" }),
    "开发服务器",
  );
  expect(screen.getByText(exited.command)).toBeTruthy();
  expect(screen.queryByText(running.command)).toBeNull();
});

test("后台任务 output continues from returned cursors and shows the final exit code", async () => {
  noPolling();
  const user = userEvent.setup();
  const chunks = [
    {
      stdout: "first\n",
      stderr: "warn\n",
      next_stdout_cursor: 6,
      next_stderr_cursor: 5,
      status: "running",
      exit_code: null,
      stdout_has_more: true,
    },
    {
      stdout: "second\n",
      stderr: "",
      next_stdout_cursor: 13,
      next_stderr_cursor: 5,
      status: "exited",
      exit_code: 7,
      stdout_has_more: false,
    },
  ];
  vi.mocked(api).mockImplementation(async (command, args) => {
    if (command === "list_jobs") return { jobs: [running] } as never;
    expect(command).toBe("job");
    expect(args?.action).toBe("read");
    return {
      error: null,
      stdout_truncated: false,
      stderr_truncated: false,
      stderr_has_more: false,
      ...chunks.shift(),
    } as never;
  });
  render(<Jobs refreshKey={0} notify={vi.fn()} />);
  await screen.findByText(running.command);
  await user.click(
    screen.getByRole("button", { name: "查看后台任务 job_running" }),
  );
  await waitFor(() =>
    expect(screen.getByLabelText("后台任务标准输出").textContent).toBe(
      "first\nsecond\n",
    ),
  );
  expect(screen.getByLabelText("后台任务错误输出").textContent).toBe("warn\n");
  expect(
    within(screen.getByRole("dialog")).getByText(/已退出.*7/),
  ).toBeTruthy();
  expect(api).toHaveBeenCalledWith("job", {
    serverId: "server-a",
    jobId: "job_running",
    action: "read",
    stdoutCursor: 6,
    stderrCursor: 5,
  });
});

test("closing a 后台任务 requires confirmation and refreshes the list after closing", async () => {
  noPolling();
  const user = userEvent.setup(),
    notify = vi.fn();
  let closed = false;
  vi.mocked(api).mockImplementation(async (command, args) => {
    if (command === "list_jobs")
      return {
        jobs: [closed ? { ...running, status: "closed" } : running],
      } as never;
    expect(args).toEqual({
      serverId: "server-a",
      jobId: "job_running",
      action: "close",
    });
    closed = true;
    return { status: "closed" } as never;
  });
  render(<Jobs refreshKey={0} notify={notify} />);
  await screen.findByText(running.command);
  await user.click(
    screen.getByRole("button", { name: "关闭后台任务 job_running" }),
  );
  await user.click(screen.getByRole("button", { name: "取消" }));
  expect(closed).toBe(false);
  await user.click(
    screen.getByRole("button", { name: "关闭后台任务 job_running" }),
  );
  await user.click(screen.getByRole("button", { name: "关闭会话" }));
  await waitFor(() =>
    expect(notify).toHaveBeenCalledWith("后台任务会话已关闭"),
  );
  expect(closed).toBe(true);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("failed 后台任务 loading can be retried", async () => {
  noPolling();
  const user = userEvent.setup();
  vi.mocked(api)
    .mockRejectedValueOnce(new Error("读取失败"))
    .mockResolvedValue({ jobs: [running] });
  render(<Jobs refreshKey={0} notify={vi.fn()} />);
  expect((await screen.findByRole("alert")).textContent).toContain("读取失败");
  await user.click(screen.getByRole("button", { name: "重试加载" }));
  await screen.findByText(running.command);
});

test("incomplete UTF-8 output waits for more bytes instead of spinning on the same cursor", async () => {
  noPolling();
  const user = userEvent.setup();
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "list_jobs") return { jobs: [running] } as never;
    return {
      stdout: "",
      stderr: "",
      next_stdout_cursor: 0,
      next_stderr_cursor: 0,
      status: "running",
      exit_code: null,
      error: null,
      stdout_truncated: false,
      stderr_truncated: false,
      stdout_has_more: true,
      stderr_has_more: false,
    } as never;
  });
  render(<Jobs refreshKey={0} notify={vi.fn()} />);
  await screen.findByText(running.command);
  await user.click(
    screen.getByRole("button", { name: "查看后台任务 job_running" }),
  );
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(
    vi.mocked(api).mock.calls.filter(([command]) => command === "job"),
  ).toHaveLength(1);
});
