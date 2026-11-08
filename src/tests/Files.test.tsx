// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import {
  cleanup,
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Files } from "../components/Files";
import { api, operate } from "../api";
import { defaultRuntimeSettings } from "../runtime-settings";
import { newServer } from "../types";
import { TransferPanel } from "../components/TransferPanel";
import type { FileTransfer } from "../transfers";

vi.mock("../api", () => ({ desktop: true, api: vi.fn(), operate: vi.fn() }));

test("transfer controls pause, resume, retry and persist priority without replaying file selection", async () => {
  const tasks: FileTransfer[] = ["active", "failed"].map((id) => ({
    transfer_id: id,
    server_id: "server",
    direction: "download",
    local_path: `C:/Downloads/${id}`,
    remote_path: `/workspace/${id}`,
    status: id === "active" ? "running" : "failed",
    phase: id === "active" ? "transferring" : "failed",
    transferred_bytes: 512,
    total_bytes: 1024,
    bytes_per_second: 256,
    elapsed_ms: 1000,
    version: 1,
    error: null,
    group_id: "group",
  }));
  vi.mocked(api).mockImplementation(async (command, args = {}) => {
    expect(command).toBe("transfer");
    if (args.action === "list")
      return { transfers: tasks.map((t) => ({ ...t })) } as never;
    const task = tasks.find((t) => t.transfer_id === args.transferId)!;
    if (args.action === "pause") task.status = "paused";
    if (args.action === "resume" || args.action === "retry")
      task.status = "running";
    if (args.action === "configure") task.priority = Number(args.priority);
    task.phase = task.status === "running" ? "transferring" : task.status;
    return { ...task } as never;
  });
  const user = userEvent.setup();
  render(
    <TransferPanel serverId="server" onComplete={vi.fn()} notify={vi.fn()} />,
  );
  await user.click(screen.getByRole("button", { name: "传输任务" }));
  await user.click(await screen.findByRole("button", { name: "暂停 active" }));
  await user.click(await screen.findByRole("button", { name: "继续 active" }));
  await user.click(screen.getByRole("button", { name: "重试 failed" }));
  const row = screen
    .getByText("active")
    .closest(".file-transfer-item") as HTMLElement;
  await user.click(within(row).getByText("传输设置"));
  fireEvent.change(within(row).getByLabelText("优先级 active"), {
    target: { value: "5" },
  });
  await waitFor(() =>
    expect(api).toHaveBeenCalledWith("transfer", {
      serverId: "server",
      action: "configure",
      transferId: "active",
      priority: 5,
    }),
  );
  expect(screen.getAllByText(/批次 group/).length).toBeGreaterThan(0);
  expect(api).toHaveBeenCalledWith("transfer", {
    serverId: "server",
    action: "resume",
    transferId: "active",
  });
  expect(api).toHaveBeenCalledWith("transfer", {
    serverId: "server",
    action: "retry",
    transferId: "failed",
  });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.resetAllMocks();
  localStorage.clear();
});

test("automatically refreshes directory metadata without overwriting input or overlapping requests", async () => {
  vi.useFakeTimers();
  localStorage.setItem(
    "remote-desk.preferences.v1",
    JSON.stringify({ refreshSeconds: "3" }),
  );
  vi.mocked(api).mockImplementation(
    async (command) =>
      (command === "runtime_settings"
        ? defaultRuntimeSettings
        : { transfers: [] }) as never,
  );
  vi.mocked(operate).mockResolvedValue({ path: "/workspace", entries: [] });
  let view: ReturnType<typeof render>;
  await act(async () => {
    view = render(
      <Files
        server={{ ...newServer(), id: "server", root: "/workspace" }}
        refresh={vi.fn()}
        notify={vi.fn()}
      />,
    );
  });
  fireEvent.change(screen.getByRole("textbox", { name: "远程目录" }), {
    target: { value: "/typing-a-new-path" },
  });
  let finish!: (result: unknown) => void;
  vi.mocked(operate).mockReturnValueOnce(
    new Promise((resolve) => {
      finish = resolve;
    }) as never,
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(3000);
  });
  expect(operate).toHaveBeenCalledTimes(2);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(6000);
  });
  expect(operate).toHaveBeenCalledTimes(2);
  await act(async () => {
    finish({
      path: "/workspace",
      entries: [
        {
          name: "new.txt",
          path: "/workspace/new.txt",
          is_dir: false,
          size: 10,
          modified: 0,
          permissions: "644",
        },
      ],
    });
  });
  expect(screen.getByRole("button", { name: "new.txt" })).toBeTruthy();
  expect(
    (screen.getByRole("textbox", { name: "远程目录" }) as HTMLInputElement)
      .value,
  ).toBe("/typing-a-new-path");
  view!.unmount();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(6000);
  });
  expect(operate).toHaveBeenCalledTimes(2);
});

test("native transfers pass only file-selection parameters and never file content", async () => {
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "runtime_settings")
      return { ...defaultRuntimeSettings } as never;
    if (command === "transfer") return { transfers: [] } as never;
    return { transfer_id: "transfer_test", status: "queued" } as never;
  });
  vi.mocked(operate).mockResolvedValue({
    path: "/workspace",
    entries: [
      {
        name: "archive.bin",
        path: "/workspace/archive.bin",
        is_dir: false,
        size: 1024,
        permissions: "644",
        modified: 0,
      },
    ],
  });
  const user = userEvent.setup();
  const view = render(
    <Files
      server={{ ...newServer(), id: "server", root: "/workspace" }}
      refresh={vi.fn()}
      notify={vi.fn()}
    />,
  );
  const row = (
    await screen.findByRole("button", { name: "archive.bin" })
  ).closest("tr")!;
  await user.click(screen.getByRole("button", { name: "上传" }));
  await waitFor(() =>
    expect(api).toHaveBeenCalledWith("upload_file", {
      serverId: "server",
      remoteDir: "/workspace",
    }),
  );
  await user.click(within(row).getByTitle("下载"));
  await waitFor(() =>
    expect(api).toHaveBeenCalledWith("download_file", {
      serverId: "server",
      path: "/workspace/archive.bin",
      name: "archive.bin",
    }),
  );
  expect(view.container.querySelector('input[type="file"]')).toBeNull();
  expect(
    vi.mocked(operate).mock.calls.every(([op]) => op.kind === "list_files"),
  ).toBe(true);
});

test("shows transfer history only after opening the toolbar transfer menu", async () => {
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "runtime_settings")
      return { ...defaultRuntimeSettings } as never;
    if (command === "transfer")
      return {
        transfers: [
          {
            transfer_id: "transfer_active",
            server_id: "server",
            direction: "download",
            local_path: "C:\\Downloads\\archive.bin",
            remote_path: "/workspace/archive.bin",
            status: "running",
            phase: "transferring",
            transferred_bytes: 512,
            total_bytes: 1024,
            bytes_per_second: 256,
            elapsed_ms: 2000,
            version: 2,
            error: null,
          },
          {
            transfer_id: "transfer_completed",
            server_id: "server",
            direction: "download",
            local_path: "C:\\Downloads\\finished.zip",
            remote_path: "/workspace/finished.zip",
            status: "completed",
            phase: "completed",
            transferred_bytes: 2048,
            total_bytes: 2048,
            bytes_per_second: 1024,
            elapsed_ms: 2000,
            version: 3,
            error: null,
          },
        ],
      } as never;
    return null as never;
  });
  vi.mocked(operate).mockResolvedValue({ path: "/workspace", entries: [] });
  const user = userEvent.setup();
  const view = render(
    <Files
      server={{ ...newServer(), id: "server", root: "/workspace" }}
      refresh={vi.fn()}
      notify={vi.fn()}
    />,
  );

  const trigger = await screen.findByRole("button", { name: "传输任务" });
  await waitFor(() =>
    expect(trigger.querySelector(".transfer-menu-badge")).toBeTruthy(),
  );
  expect(view.container.querySelector(".file-transfer-item")).toBeNull();

  await user.click(trigger);
  const task = await screen.findByText("archive.bin");
  const menu = task.closest(".transfer-menu") as HTMLElement;
  expect(menu).toBeTruthy();
  expect(within(menu).getByText(/512 B \/ 1.0 KiB · 256 B\/s/)).toBeTruthy();
  const completed = within(menu)
    .getByText("finished.zip")
    .closest(".file-transfer-item") as HTMLElement;
  expect(completed.querySelector("progress")).toBeNull();
  await user.click(
    within(completed).getByRole("button", {
      name: "打开 finished.zip 所在文件夹",
    }),
  );
  expect(api).toHaveBeenCalledWith("reveal_transfer", {
    serverId: "server",
    transferId: "transfer_completed",
  });
});
