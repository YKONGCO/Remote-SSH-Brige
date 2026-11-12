// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { Files } from "../components/Files";
import { api, operate } from "../api";
import { defaultRuntimeSettings } from "../runtime-settings";
import { newServer } from "../types";

vi.mock("../api", () => ({ desktop: false, api: vi.fn(), operate: vi.fn() }));

beforeAll(() => {
  Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
  localStorage.clear();
});

test("dropping files uploads them to the current directory", async () => {
  vi.mocked(api).mockImplementation(async (command) =>
    command === "runtime_settings"
      ? ({ ...defaultRuntimeSettings } as never)
      : ({ transfers: [] } as never),
  );
  vi.mocked(operate).mockImplementation(async (op) => {
    if (op.kind === "list_files")
      return { path: "/workspace", entries: [] } as never;
    return {} as never;
  });
  const notify = vi.fn();
  const { container } = render(
    <Files
      server={{ ...newServer(), id: "server", root: "/workspace" }}
      refresh={vi.fn()}
      notify={notify}
    />,
  );
  const workspace = container.querySelector(".files-workspace")!;
  const file = new File(["hello"], "hello.txt", { type: "text/plain" });
  fireEvent.dragEnter(workspace, { dataTransfer: { types: ["Files"] } });
  expect(screen.getByText("松开鼠标上传")).toBeTruthy();
  fireEvent.drop(workspace, {
    dataTransfer: { types: ["Files"], files: [file] },
  });
  await waitFor(() =>
    expect(operate).toHaveBeenCalledWith({
      kind: "upload",
      server_id: "server",
      path: "/workspace/hello.txt",
      content: "aGVsbG8=",
    }),
  );
  expect(notify).toHaveBeenCalledWith("已上传 1 个文件", false);
  expect(screen.queryByText("松开鼠标上传")).toBeNull();
});

test("dropping files is rejected in read-only mode", async () => {
  vi.mocked(api).mockImplementation(async (command) =>
    command === "runtime_settings"
      ? ({ ...defaultRuntimeSettings, remote_read_only: true } as never)
      : ({ transfers: [] } as never),
  );
  vi.mocked(operate).mockResolvedValue({ path: "/workspace", entries: [] });
  const notify = vi.fn();
  const { container } = render(
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
  const workspace = container.querySelector(".files-workspace")!;
  const file = new File(["hello"], "hello.txt", { type: "text/plain" });
  fireEvent.dragEnter(workspace, { dataTransfer: { types: ["Files"] } });
  expect(screen.queryByText("松开鼠标上传")).toBeNull();
  fireEvent.drop(workspace, {
    dataTransfer: { types: ["Files"], files: [file] },
  });
  expect(notify).toHaveBeenCalledWith("只读模式下不能上传文件", true);
  expect(operate).not.toHaveBeenCalledWith(
    expect.objectContaining({ kind: "upload" }),
  );
});
