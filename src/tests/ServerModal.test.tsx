// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ServerModal } from "../components/ServerModal";
import { newServer } from "../types";
import { api } from "../api";
import { defaultRuntimeSettings } from "../runtime-settings";
vi.mock("../api", () => ({ api: vi.fn() }));
beforeEach(() => {
  vi.mocked(api).mockResolvedValue({
    ...defaultRuntimeSettings,
    deletion_protection: false,
  });
});

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

const mount = (permissions: string[] = [], onOpenSettings?: () => void) => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(
    <ServerModal
      initial={{ ...initial, mcp_permissions: permissions }}
      onClose={vi.fn()}
      onSave={save}
      onOpenSettings={onOpenSettings}
    />,
  );
  return save;
};
const box = (name: string) => screen.getByRole("checkbox", { name });
const submit = async (
  user: ReturnType<typeof userEvent.setup>,
  save: ReturnType<typeof vi.fn>,
) => {
  await user.click(screen.getByRole("button", { name: "保存服务器" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  return save.mock.calls[0][0].mcp_permissions as string[];
};
test("eight categories replace individual tool switches and master switch disables controls", async () => {
  const user = userEvent.setup();
  mount();
  expect(screen.getAllByRole("checkbox")).toHaveLength(9);
  expect(screen.getAllByRole("button", { name: /全选/ })).toHaveLength(1);
  await user.click(box("允许 MCP 访问此服务器"));
  for (const control of screen.getAllByRole("checkbox").slice(1))
    expect((control as HTMLButtonElement).disabled).toBe(true);
  expect(
    (screen.getByRole("button", { name: "全选" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
});
test("writing grants read and edit, revoking reading revokes writing", async () => {
  const user = userEvent.setup();
  const save = mount();
  await user.click(box("文件写入"));
  expect(box("文件读取").getAttribute("aria-checked")).toBe("true");
  await user.click(box("文件读取"));
  expect(box("文件写入").getAttribute("aria-checked")).toBe("false");
  expect(await submit(user, save)).toEqual([]);
});
test("port forwarding is opt-in and grants its four independent actions", async () => {
  const user = userEvent.setup();
  const save = mount();
  expect(box("端口转发").getAttribute("aria-checked")).toBe("false");
  await user.click(box("端口转发"));
  expect(await submit(user, save)).toEqual(
    expect.arrayContaining([
      "port_forward_open",
      "port_forward_list",
      "port_forward_read",
      "port_forward_close",
    ]),
  );
});
test("writing and command categories include dependencies and jobs", async () => {
  const user = userEvent.setup();
  const save = mount();
  await user.click(box("文件写入"));
  await user.click(box("命令执行"));
  expect(await submit(user, save)).toEqual(
    expect.arrayContaining([
      "read_file",
      "write_file",
      "edit_file",
      "exec",
      "job",
    ]),
  );
});
test("partial legacy grants remain unchanged when saving unrelated fields", async () => {
  const user = userEvent.setup();
  const grants = ["terminal_read", "exec", "read_file", "write_file"];
  const save = mount(grants);
  expect(box("终端操作").getAttribute("aria-checked")).toBe("mixed");
  expect(box("命令执行").getAttribute("aria-checked")).toBe("mixed");
  expect(await submit(user, save)).toEqual(grants);
});
test("explicitly selecting a partial terminal category grants all actions", async () => {
  const user = userEvent.setup();
  const save = mount(["terminal_read"]);
  await user.click(box("终端操作"));
  expect(await submit(user, save)).toEqual(
    expect.arrayContaining([
      "list_terminals",
      "terminal_open",
      "terminal_read",
      "terminal_write",
      "terminal_resize",
      "terminal_close",
    ]),
  );
});
test.each(["文件上传", "文件下载"])(
  "%s independently adds transfer management",
  async (label) => {
    const user = userEvent.setup();
    const save = mount();
    await user.click(box(label));
    expect(await submit(user, save)).toEqual([
      label === "文件上传" ? "upload_file" : "download_file",
      "transfer",
    ]);
  },
);
test("removing one transfer direction preserves the other and management", async () => {
  const user = userEvent.setup();
  const save = mount(["upload_file", "download_file", "transfer"]);
  await user.click(box("文件上传"));
  expect(await submit(user, save)).toEqual(["download_file", "transfer"]);
});
test("deletion protection preserves existing grant, disables toggle and offers settings link", async () => {
  vi.mocked(api).mockResolvedValue({
    ...defaultRuntimeSettings,
    deletion_protection: true,
  });
  const user = userEvent.setup();
  const open = vi.fn();
  const save = mount(["remove_path"], open);
  await screen.findByText("已授权，受全局删除保护限制");
  expect((box("文件删除") as HTMLButtonElement).disabled).toBe(true);
  await user.click(screen.getByRole("button", { name: "前往删除保护设置" }));
  expect(open).toHaveBeenCalledOnce();
  await user.click(screen.getByRole("button", { name: "全选" }));
  await user.click(screen.getByRole("button", { name: "取消全选" }));
  expect(await submit(user, save)).toEqual(["remove_path"]);
});
test("select all cannot grant deletion under protection", async () => {
  vi.mocked(api).mockResolvedValue({
    ...defaultRuntimeSettings,
    deletion_protection: true,
  });
  const user = userEvent.setup();
  const save = mount();
  await user.click(screen.getByRole("button", { name: "全选" }));
  const grants = await submit(user, save);
  expect(grants).not.toContain("remove_path");
  expect(grants).toContain("job");
});
test("deletion is independently grantable after protection is disabled", async () => {
  const user = userEvent.setup();
  const save = mount();
  await waitFor(() =>
    expect((box("文件删除") as HTMLButtonElement).disabled).toBe(false),
  );
  await user.click(box("文件删除"));
  expect(await submit(user, save)).toEqual(["remove_path"]);
});
test("settings load failure fails closed for deletion and preserves permissions", async () => {
  vi.mocked(api).mockRejectedValue(new Error("offline"));
  const user = userEvent.setup();
  const save = mount(["remove_path"]);
  await screen.findByRole("alert");
  expect((box("文件删除") as HTMLButtonElement).disabled).toBe(true);
  expect(await submit(user, save)).toEqual(["remove_path"]);
});
test("legacy master switches convert without adding grouped capabilities", async () => {
  const user = userEvent.setup();
  const save = vi.fn().mockResolvedValue(undefined);
  render(
    <ServerModal
      initial={{
        ...initial,
        mcp_permissions: null,
        allow_write: true,
        allow_exec: true,
      }}
      onClose={vi.fn()}
      onSave={save}
    />,
  );
  expect(await submit(user, save)).toEqual([
    "read_file",
    "list_files",
    "stat_file",
    "write_file",
    "exec",
  ]);
});
