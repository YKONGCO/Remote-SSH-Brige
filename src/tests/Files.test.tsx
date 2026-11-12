// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { EditorView } from "@codemirror/view";
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

type DropHandlers = {
  onEnter: (paths: string[]) => void;
  onLeave: () => void;
  onDrop: (paths: string[]) => void;
};
const nativeDrop = vi.hoisted(() => ({
  available: false,
  handlers: null as DropHandlers | null,
  unlisten: vi.fn(),
}));
vi.mock("../native-drop", () => ({
  nativeDragDropAvailable: () => nativeDrop.available,
  listenForNativeDrops: async (handlers: DropHandlers) => {
    nativeDrop.handlers = handlers;
    return nativeDrop.unlisten;
  },
}));

beforeAll(() => {
  Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
});

const codeEntry = {
  name: "app.ts",
  path: "/workspace/app.ts",
  is_dir: false,
  size: 128,
  modified: 0,
  permissions: "644",
};
const codePreview = {
  kind: "text",
  content: "const answer = 42;\n",
  bytes: 128,
  truncated: false,
};

function setupFilePreview(
  options: { truncated?: boolean; readOnly?: boolean } = {},
) {
  vi.mocked(api).mockImplementation(async (command) =>
    command === "runtime_settings"
      ? ({
          ...defaultRuntimeSettings,
          remote_read_only: !!options.readOnly,
        } as never)
      : (null as never),
  );
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "list_files")
      return { path: "/workspace", entries: [codeEntry] } as never;
    if (op.kind === "read_file")
      return (
        op.file_request
          ? {
              ...codePreview,
              truncated: !!options.truncated,
              preview_limit_bytes: 20 * 1024 * 1024,
            }
          : { content: "complete file\n" }
      ) as never;
    return {} as never;
  });
  return render(
    <Files
      server={{ ...newServer(), id: "server", root: "/workspace" }}
      refresh={vi.fn()}
      notify={vi.fn()}
    />,
  );
}

async function changeCode(value: string) {
  const element = await screen.findByRole("textbox", { name: "文件内容" });
  const view = EditorView.findFromDOM(element)!;
  act(() =>
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value },
    }),
  );
  return view;
}

test("preview and editing share one dialog, document and undo history without rereading", async () => {
  const user = userEvent.setup();
  setupFilePreview();
  await user.click(await screen.findByRole("button", { name: "app.ts" }));
  const dialog = screen.getByRole("dialog");
  const preview = await screen.findByRole("textbox", { name: "文件预览" });
  const view = EditorView.findFromDOM(preview)!;
  expect(view.state.readOnly).toBe(true);
  await user.click(screen.getByRole("button", { name: "编辑" }));
  await changeCode("const answer = 43;\n");
  await user.click(screen.getByRole("button", { name: "预览" }));
  expect(
    screen.getByRole("textbox", { name: "文件预览" }).textContent,
  ).toContain("43");
  await user.click(screen.getByRole("button", { name: "编辑" }));
  expect(
    EditorView.findFromDOM(screen.getByRole("textbox", { name: "文件内容" })),
  ).toBe(view);
  expect(screen.getAllByRole("dialog")).toEqual([dialog]);
  await user.keyboard("{Control>}z{/Control}");
  expect(view.state.sliceDoc()).toBe(codePreview.content);
  await user.keyboard("{Control>}{Shift>}z{/Shift}{/Control}");
  expect(view.state.sliceDoc()).toBe("const answer = 43;\n");
  expect(
    vi.mocked(operate).mock.calls.filter(([op]) => op.kind === "read_file"),
  ).toHaveLength(1);
  await user.keyboard("{Control>}s{/Control}");
  await waitFor(() =>
    expect(operate).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "write_file",
        path: codeEntry.path,
        content: "const answer = 43;\n",
      }),
    ),
  );
  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "保存更改" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true),
  );
  await user.click(screen.getByRole("button", { name: "关闭文件预览" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("unsaved changes are protected in the same dialog and failed saves retain the draft", async () => {
  const user = userEvent.setup();
  setupFilePreview();
  const normal = vi.mocked(operate).getMockImplementation()!;
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "write_file") throw new Error("permission denied");
    return normal(op);
  });
  await user.click(await screen.findByRole("button", { name: "app.ts" }));
  await user.click(await screen.findByRole("button", { name: "编辑" }));
  await changeCode("unsaved draft");
  await user.keyboard("{Escape}");
  expect(screen.getByText("修改尚未保存")).toBeTruthy();
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
  await user.click(screen.getByRole("button", { name: "保存并关闭" }));
  expect(await screen.findByText(/permission denied/)).toBeTruthy();
  expect(
    screen.getByRole("textbox", { name: "文件预览" }).textContent,
  ).toContain("unsaved draft");
  await user.click(screen.getByRole("button", { name: "继续编辑" }));
  expect(
    (await screen.findByRole("textbox", { name: "文件内容" })).textContent,
  ).toContain("unsaved draft");
  await user.click(screen.getByRole("button", { name: "关闭文件预览" }));
  await user.click(screen.getByRole("button", { name: "放弃修改" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("truncated previews cannot be saved and editing requires a successful full read", async () => {
  const user = userEvent.setup();
  setupFilePreview({ truncated: true });
  const normal = vi.mocked(operate).getMockImplementation()!;
  let failRead = true;
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "read_file" && !op.file_request && failRead)
      throw new Error("read failed");
    return normal(op);
  });
  await user.click(await screen.findByRole("button", { name: "app.ts" }));
  expect(await screen.findByText(/仅预览前 20/)).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "编辑" }));
  expect(await screen.findByText(/read failed/)).toBeTruthy();
  expect(screen.queryByRole("textbox", { name: "文件内容" })).toBeNull();
  expect(
    (screen.getByRole("button", { name: "保存更改" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  failRead = false;
  await user.click(screen.getByRole("button", { name: "编辑" }));
  const editor = await screen.findByRole("textbox", { name: "文件内容" });
  expect(editor.textContent).toContain("complete file");
  await changeCode("complete updated file\n");
  await user.click(screen.getByRole("button", { name: "保存更改" }));
  await waitFor(() =>
    expect(operate).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "write_file",
        content: "complete updated file\n",
      }),
    ),
  );
});

test("closing a loading preview ignores its late response and errors can be retried", async () => {
  const user = userEvent.setup();
  setupFilePreview();
  const normal = vi.mocked(operate).getMockImplementation()!;
  let finish: (value: unknown) => void = () => {};
  vi.mocked(operate).mockImplementation(async (op) =>
    op.kind === "read_file"
      ? new Promise((resolve) => {
          finish = resolve;
        })
      : normal(op),
  );
  await user.click(await screen.findByRole("button", { name: "app.ts" }));
  expect(screen.getByText("正在读取远程文件…")).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "关闭文件预览" }));
  await act(async () => finish(codePreview));
  expect(screen.queryByRole("dialog")).toBeNull();
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "read_file") throw new Error("offline");
    return normal(op);
  });
  await user.click(screen.getByRole("button", { name: "app.ts" }));
  expect(await screen.findByText(/offline/)).toBeTruthy();
  vi.mocked(operate).mockImplementation(normal);
  await user.click(screen.getByRole("button", { name: "重试" }));
  expect(await screen.findByRole("textbox", { name: "文件预览" })).toBeTruthy();
});

test("read-only mode hides editing and saving controls", async () => {
  const user = userEvent.setup();
  setupFilePreview({ readOnly: true });
  await user.click(await screen.findByRole("button", { name: "app.ts" }));
  const preview = await screen.findByRole("textbox", { name: "文件预览" });
  expect(EditorView.findFromDOM(preview)!.state.readOnly).toBe(true);
  expect(screen.queryByRole("button", { name: "编辑" })).toBeNull();
  expect(screen.queryByRole("button", { name: "保存更改" })).toBeNull();
  await user.keyboard("{Control>}s{/Control}");
  expect(
    vi.mocked(operate).mock.calls.some(([op]) => op.kind === "write_file"),
  ).toBe(false);
});

test("Escape closes editor search before closing the file dialog", async () => {
  const user = userEvent.setup();
  setupFilePreview();
  await user.click(await screen.findByRole("button", { name: "app.ts" }));
  const preview = await screen.findByRole("textbox", { name: "文件预览" });
  act(() => preview.focus());
  await user.keyboard("{Control>}f{/Control}");
  expect(await screen.findByRole("textbox", { name: "查找" })).toBeTruthy();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("textbox", { name: "查找" })).toBeNull();
  expect(screen.getByRole("dialog")).toBeTruthy();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
});

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
  await user.click(
    await screen.findByRole("button", { name: /^批次 group，/ }),
  );
  await user.click(await screen.findByRole("button", { name: "暂停 active" }));
  await user.click(await screen.findByRole("button", { name: "继续 active" }));
  await user.click(screen.getByRole("button", { name: "重试 failed" }));
  const row = screen
    .getByText("active")
    .closest(".file-transfer-item") as HTMLElement;
  await user.click(
    within(row).getByRole("button", { name: "传输设置 active" }),
  );
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
  nativeDrop.available = false;
  nativeDrop.handlers = null;
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
    if (command === "pick_transfer_batch")
      return {
        tasks: [{ transfer_id: "batch_test" }],
        queued: 1,
        rejected: 0,
      } as never;
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
  expect(screen.queryByRole("button", { name: "批量上传" })).toBeNull();
  expect(screen.queryByRole("button", { name: "上传文件夹" })).toBeNull();
  expect(screen.queryByRole("menuitem", { name: "上传文件" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "上传" }));
  expect(api).not.toHaveBeenCalledWith("upload_file", expect.anything());
  await user.click(screen.getByRole("menuitem", { name: "上传文件" }));
  await waitFor(() =>
    expect(api).toHaveBeenCalledWith("upload_file", {
      serverId: "server",
      remoteDir: "/workspace",
    }),
  );
  for (const directory of [false, true]) {
    await user.click(screen.getByRole("button", { name: "上传" }));
    await user.click(
      screen.getByRole("menuitem", {
        name: directory ? "上传文件夹" : "批量上传",
      }),
    );
    await waitFor(() =>
      expect(api).toHaveBeenCalledWith("pick_transfer_batch", {
        serverId: "server",
        remotePath: "/workspace",
        direction: "upload",
        directory,
      }),
    );
    expect(screen.queryByRole("menu")).toBeNull();
  }
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

function makeTransfer(index: number, group?: string): FileTransfer {
  return {
    transfer_id: `transfer-${index}`,
    server_id: "server",
    direction: "download",
    local_path: `C:/Downloads/file-${String(index).padStart(3, "0")}.txt`,
    remote_path: `/workspace/file-${String(index).padStart(3, "0")}.txt`,
    status: "completed",
    phase: "completed",
    transferred_bytes: 1024,
    total_bytes: 1024,
    bytes_per_second: 0,
    elapsed_ms: 1000,
    version: 1,
    error: null,
    group_id: group,
  };
}

function renderTransfers(items: FileTransfer[]) {
  vi.mocked(api).mockImplementation(
    async () => ({ transfers: items.map((item) => ({ ...item })) }) as never,
  );
  return render(
    <TransferPanel serverId="server" onComplete={vi.fn()} notify={vi.fn()} />,
  );
}

function scrollTransfersToBottom() {
  const body = screen
    .getByRole("dialog", { name: "文件传输任务" })
    .querySelector(".transfer-menu-body") as HTMLElement;
  Object.defineProperties(body, {
    scrollHeight: { configurable: true, value: 2500 },
    clientHeight: { configurable: true, value: 500 },
    scrollTop: { configurable: true, writable: true, value: 2000 },
  });
  fireEvent.scroll(body);
}

test("each batch expands independently and grouped files never appear as standalone cards", async () => {
  const user = userEvent.setup();
  renderTransfers([
    makeTransfer(0, "demo-500"),
    makeTransfer(1, "demo-500"),
    makeTransfer(2, "other-batch"),
    makeTransfer(3),
  ]);
  await user.click(screen.getByRole("button", { name: "传输任务" }));
  const first = await screen.findByRole("button", { name: /^批次 demo-500，/ });
  const other = screen.getByRole("button", { name: /^批次 other-batch，/ });
  expect(first.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByText("file-000.txt")).toBeNull();
  expect(screen.getByText("file-003.txt")).toBeTruthy();
  expect(screen.queryByText("查看传输进度与历史记录")).toBeNull();
  expect(screen.queryByRole("progressbar")).toBeNull();
  await user.click(first);
  expect(screen.getAllByText("file-000.txt")).toHaveLength(1);
  expect(screen.getAllByText("file-001.txt")).toHaveLength(1);
  expect(other.getAttribute("aria-expanded")).toBe("false");
  await user.click(other);
  await user.click(first);
  expect(screen.queryByText("file-000.txt")).toBeNull();
  expect(screen.getByText("file-002.txt")).toBeTruthy();
  expect(
    screen
      .getByText("file-003.txt")
      .closest(".file-transfer-item")!
      .classList.contains("is-batch-child"),
  ).toBe(false);
});

test("scrolling loads all standalone transfers and all batches beyond the old limits", async () => {
  const user = userEvent.setup();
  const view = renderTransfers(
    Array.from({ length: 95 }, (_, index) => makeTransfer(index)),
  );
  await user.click(screen.getByRole("button", { name: "传输任务" }));
  await screen.findByText("file-000.txt");
  expect(screen.queryByText("file-094.txt")).toBeNull();
  for (let page = 0; page < 3; page++) scrollTransfersToBottom();
  expect(screen.getByText("file-094.txt")).toBeTruthy();
  expect(document.querySelectorAll(".file-transfer-item")).toHaveLength(95);
  expect(screen.queryByText("向下滚动加载更多")).toBeNull();
  view.unmount();

  renderTransfers(
    Array.from({ length: 45 }, (_, index) =>
      makeTransfer(index, `batch-${index}`),
    ),
  );
  await user.click(screen.getByRole("button", { name: "传输任务" }));
  await screen.findByRole("button", { name: /^批次 batch-0，/ });
  expect(screen.queryByRole("button", { name: /^批次 batch-44，/ })).toBeNull();
  scrollTransfersToBottom();
  expect(screen.getByRole("button", { name: /^批次 batch-44，/ })).toBeTruthy();
  expect(document.querySelectorAll(".transfer-batch")).toHaveLength(45);
  expect(document.querySelectorAll(".file-transfer-item")).toHaveLength(0);
});

test("expanded large batches load all their files on scroll and search reaches unloaded files", async () => {
  const user = userEvent.setup();
  renderTransfers(
    Array.from({ length: 95 }, (_, index) =>
      makeTransfer(index, "large-batch"),
    ),
  );
  await user.click(screen.getByRole("button", { name: "传输任务" }));
  await user.click(
    await screen.findByRole("button", { name: /^批次 large-batch，/ }),
  );
  expect(screen.getByText("file-000.txt")).toBeTruthy();
  expect(screen.queryByText("file-094.txt")).toBeNull();
  for (let page = 0; page < 3; page++) scrollTransfersToBottom();
  expect(screen.getByText("file-094.txt")).toBeTruthy();
  expect(
    document.querySelectorAll(".file-transfer-item.is-batch-child"),
  ).toHaveLength(95);
  await user.click(screen.getByRole("button", { name: /^批次 large-batch，/ }));
  expect(screen.queryByText("file-094.txt")).toBeNull();
  await user.click(screen.getByRole("button", { name: "搜索传输任务" }));
  await user.type(
    screen.getByRole("textbox", { name: "搜索传输任务" }),
    "file-094",
  );
  expect(
    screen
      .getByRole("button", { name: /^批次 large-batch，/ })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  expect(screen.getByText("file-094.txt")).toBeTruthy();
  expect(document.querySelectorAll(".file-transfer-item")).toHaveLength(1);
});

test("expanding the last loaded batch reveals children and active progress stays bounded", async () => {
  const user = userEvent.setup();
  const items = Array.from({ length: 31 }, (_, index) => ({
    ...makeTransfer(index, `batch-${index}`),
    status: "running" as const,
    phase: "transferring",
    transferred_bytes: 2048,
  }));
  renderTransfers(items);
  await user.click(screen.getByRole("button", { name: "传输任务" }));
  const last = await screen.findByRole("button", { name: /^批次 batch-29，/ });
  expect(screen.queryByText("file-029.txt")).toBeNull();
  const progress = screen.getByRole("progressbar", {
    name: "批次 batch-29 进度",
  }) as HTMLProgressElement;
  expect(progress.value).toBe(progress.max);
  await user.click(last);
  expect(screen.getByText("file-029.txt")).toBeTruthy();
  expect(screen.getByRole("button", { name: /^批次 batch-30，/ })).toBeTruthy();
});

test("native drops submit the dropped paths for the current directory", async () => {
  nativeDrop.available = true;
  const notify = vi.fn();
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "runtime_settings")
      return { ...defaultRuntimeSettings } as never;
    if (command === "upload_paths")
      return {
        tasks: [{ transfer_id: "a" }, { transfer_id: "b" }],
        queued: 2,
        rejected: 0,
      } as never;
    if (command === "transfer") return { transfers: [] } as never;
    return null as never;
  });
  vi.mocked(operate).mockResolvedValue({ path: "/workspace", entries: [] });
  render(
    <Files
      server={{ ...newServer(), id: "server", root: "/workspace" }}
      refresh={vi.fn()}
      notify={notify}
    />,
  );
  await waitFor(() => expect(nativeDrop.handlers).not.toBeNull());
  act(() => nativeDrop.handlers!.onEnter(["/tmp/a.txt", "/tmp/b.txt"]));
  expect(screen.getByText("松开鼠标上传")).toBeTruthy();
  expect(screen.getByText("上传到 /workspace")).toBeTruthy();
  act(() => nativeDrop.handlers!.onDrop(["/tmp/a.txt", "/tmp/b.txt"]));
  await waitFor(() =>
    expect(api).toHaveBeenCalledWith("upload_paths", {
      serverId: "server",
      remoteDir: "/workspace",
      paths: ["/tmp/a.txt", "/tmp/b.txt"],
    }),
  );
  expect(notify).toHaveBeenCalledWith("已提交 2 个传输任务", false);
  expect(screen.queryByText("松开鼠标上传")).toBeNull();
});

test("native drops are ignored while the server is read-only", async () => {
  nativeDrop.available = true;
  const notify = vi.fn();
  vi.mocked(api).mockImplementation(async (command) => {
    if (command === "runtime_settings")
      return { ...defaultRuntimeSettings, remote_read_only: true } as never;
    if (command === "transfer") return { transfers: [] } as never;
    return null as never;
  });
  vi.mocked(operate).mockResolvedValue({ path: "/workspace", entries: [] });
  render(
    <Files
      server={{ ...newServer(), id: "server", root: "/workspace" }}
      refresh={vi.fn()}
      notify={notify}
    />,
  );
  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "新建文件" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true),
  );
  await waitFor(() => expect(nativeDrop.handlers).not.toBeNull());
  act(() => nativeDrop.handlers!.onEnter(["/tmp/a.txt"]));
  expect(screen.queryByText("松开鼠标上传")).toBeNull();
  act(() => nativeDrop.handlers!.onDrop(["/tmp/a.txt"]));
  await waitFor(() =>
    expect(notify).toHaveBeenCalledWith("只读模式下不能上传文件", true),
  );
  expect(api).not.toHaveBeenCalledWith("upload_paths", expect.anything());
});
