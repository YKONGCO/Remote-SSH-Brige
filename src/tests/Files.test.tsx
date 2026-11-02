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

vi.mock("../api", () => ({ desktop: true, api: vi.fn(), operate: vi.fn() }));
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
