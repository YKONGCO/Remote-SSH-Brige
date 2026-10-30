// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { render, screen, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
const calls = vi.hoisted(() => ({
  minimize: vi.fn().mockResolvedValue(undefined),
  hide: vi.fn().mockResolvedValue(undefined),
  close: vi.fn().mockResolvedValue(undefined),
  toggleMaximize: vi.fn().mockResolvedValue(undefined),
  isMaximized: vi.fn().mockResolvedValue(false),
  unlisten: vi.fn(),
  onResized: vi.fn(),
}));
vi.mock("../api", () => ({ desktop: true }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    ...calls,
    onResized: calls.onResized.mockResolvedValue(calls.unlisten),
  }),
}));
import { WindowBar } from "./WindowBar";
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});
test("window close requests use the native handler while tray control always hides", async () => {
  const user = userEvent.setup();
  const onError = vi.fn();
  const { unmount } = render(<WindowBar onError={onError} />);
  await user.click(screen.getByRole("button", { name: "最小化" }));
  expect(calls.minimize).toHaveBeenCalledOnce();
  calls.isMaximized.mockResolvedValue(true);
  await user.click(screen.getByRole("button", { name: "最大化" }));
  expect(calls.toggleMaximize).toHaveBeenCalledOnce();
  await screen.findByRole("button", { name: "还原窗口" });
  await user.click(screen.getByRole("button", { name: "收起到托盘" }));
  await user.click(screen.getByRole("button", { name: "关闭窗口" }));
  expect(calls.hide).toHaveBeenCalledOnce();
  expect(calls.close).toHaveBeenCalledOnce();
  expect(onError).not.toHaveBeenCalled();
  unmount();
  expect(calls.unlisten).toHaveBeenCalledOnce();
});
test("window API failures are reported", async () => {
  const user = userEvent.setup();
  const onError = vi.fn();
  calls.hide.mockRejectedValueOnce(new Error("隐藏窗口失败"));
  render(<WindowBar onError={onError} />);
  await user.click(screen.getByRole("button", { name: "收起到托盘" }));
  await waitFor(() =>
    expect(onError).toHaveBeenCalledWith("Error: 隐藏窗口失败"),
  );
});
