// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ServerModal } from "./ServerModal";
import { newServer } from "../types";

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
afterEach(cleanup);
const initial = {
  ...newServer(),
  id: "test",
  name: "Test",
  host: "127.0.0.1",
  auth: "agent" as const,
};

test("editing requires read permission and revoking read removes edit permission", async () => {
  const user = userEvent.setup();
  const save = vi.fn().mockResolvedValue(undefined);
  render(<ServerModal initial={initial} onClose={vi.fn()} onSave={save} />);
  const read = screen.getByRole("checkbox", { name: "读取文本" });
  const edit = screen.getByRole("checkbox", { name: "精确编辑（需读取权限）" });
  expect(edit.getAttribute("aria-checked")).toBe("false");
  await user.click(edit);
  expect(edit.getAttribute("aria-checked")).toBe("true");
  await user.click(read);
  expect(edit.getAttribute("aria-checked")).toBe("false");
  expect((edit as HTMLButtonElement).disabled).toBe(true);
  await user.click(screen.getByRole("button", { name: "保存服务器" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0].mcp_permissions).not.toContain("edit_file");
});

test("file upload and terminal output can be granted independently", async () => {
  const user = userEvent.setup();
  const save = vi.fn().mockResolvedValue(undefined);
  render(<ServerModal initial={initial} onClose={vi.fn()} onSave={save} />);
  await user.click(screen.getByRole("checkbox", { name: "下载文件" }));
  await user.click(screen.getByRole("checkbox", { name: "上传文件" }));
  await user.click(screen.getByRole("checkbox", { name: "读取输出" }));
  expect(
    (
      screen.getByRole("checkbox", { name: "发送输入" }) as HTMLButtonElement
    ).getAttribute("aria-checked"),
  ).toBe("false");
  await user.click(screen.getByRole("button", { name: "保存服务器" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  const saved = save.mock.calls[0][0];
  expect(saved.mcp_permissions).toEqual(
    expect.arrayContaining(["upload", "terminal_read", "list_files"]),
  );
  for (const denied of [
    "download",
    "write_file",
    "remove",
    "terminal_write",
    "terminal_open",
  ])
    expect(saved.mcp_permissions).not.toContain(denied);
});

test("master switch preserves grants and legacy selections convert on save", async () => {
  const user = userEvent.setup();
  const save = vi.fn().mockResolvedValue(undefined);
  render(
    <ServerModal
      initial={{ ...initial, mcp_permissions: null, allow_write: true }}
      onClose={vi.fn()}
      onSave={save}
    />,
  );
  expect(
    screen
      .getByRole("checkbox", { name: "删除文件 / 目录" })
      .getAttribute("aria-checked"),
  ).toBe("true");
  await user.click(
    screen.getByRole("checkbox", { name: "允许 MCP 访问此服务器" }),
  );
  expect(
    (screen.getByRole("checkbox", { name: "下载文件" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await user.click(screen.getByRole("button", { name: "保存服务器" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0].mcp_enabled).toBe(false);
  expect(save.mock.calls[0][0].mcp_permissions).toContain("remove");
  expect(save.mock.calls[0][0].mcp_permissions).not.toContain("exec");
});
