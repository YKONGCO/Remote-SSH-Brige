// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import {
  cleanup,
  render,
  screen,
  within,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import * as remoteApi from "./api";
beforeAll(() => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
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
test("remote files wait for server selection on each visit", async () => {
  localStorage.clear();
  const user = userEvent.setup();
  const operations = vi.spyOn(remoteApi, "operate");
  try {
    render(<App />);
    await screen.findByText("生产服务器");
    await user.click(screen.getByRole("button", { name: "远程文件" }));
    expect(screen.getByText("选择一台服务器")).toBeTruthy();
    expect(operations).not.toHaveBeenCalled();

    await user.click(screen.getByRole("combobox", { name: "选择服务器" }));
    expect(
      screen.getByRole("option", { name: "测试服务器（已停用）" })
        .getAttribute("aria-disabled"),
    ).toBe("true");
    await user.click(screen.getByRole("option", { name: "开发服务器" }));
    await screen.findByRole("button", { name: "README.md" });
    expect(operations).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "list_files", server_id: "demo-2" }),
    );

    await user.click(screen.getByRole("button", { name: "服务器" }));
    operations.mockClear();
    await user.click(screen.getByRole("button", { name: "远程文件" }));
    expect(screen.getByText("选择一台服务器")).toBeTruthy();
    expect(operations).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "服务器" }));
    await user.click(screen.getByRole("button", { name: "浏览文件 生产服务器" }));
    await screen.findByRole("button", { name: "README.md" });
    expect(operations).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "list_files", server_id: "demo-1" }),
    );
  } finally {
    operations.mockRestore();
  }
});
test("workspace supports filtering, server creation, file editing and audit navigation", async () => {
  const user = userEvent.setup();
  render(<App />);
  await screen.findByText("生产服务器");
  expect(screen.getByTitle("Remote SSH Brige")).toBeTruthy();
  expect(screen.getByText("Remote SSH")).toBeTruthy();
  expect(screen.getByText("Brige")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "命令执行" })).toBeNull();
  expect(screen.queryByText("本地工作空间")).toBeNull();
  expect(screen.getByRole("button", { name: "收起到托盘" })).toBeTruthy();
  expect(screen.getByText(/界面演示模式/)).toBeTruthy();
  const search = screen.getByRole("textbox", { name: "搜索服务器" });
  await user.type(search, "192.168.1.101");
  expect(screen.queryByText("生产服务器")).toBeNull();
  expect(screen.getByText("开发服务器")).toBeTruthy();
  await user.clear(search);
  await user.click(screen.getByRole("button", { name: "添加服务器" }));
  await user.type(screen.getByLabelText("服务器名称"), "测试连接");
  await user.type(screen.getByLabelText("主机地址"), "127.0.0.2");
  await user.click(screen.getByRole("tab", { name: "账户密码" }));
  const account = screen.getByLabelText("登录账号");
  await user.clear(account);
  await user.type(account, "test-user");
  const password = screen.getByLabelText("登录密码");
  expect((password as HTMLInputElement).type).toBe("password");
  expect((password as HTMLInputElement).required).toBe(true);
  await user.type(password, "fixture-password");
  await user.click(screen.getByRole("button", { name: "保存服务器" }));
  await screen.findByText("测试连接");
  expect(
    screen.queryByText("连接你的远程环境，让工具与工作流在这里相遇。"),
  ).toBeNull();
  await user.click(screen.getByRole("button", { name: "远程文件" }));
  await user.click(screen.getByRole("combobox", { name: "选择服务器" }));
  await user.click(screen.getByRole("option", { name: "生产服务器" }));
  const readme = await screen.findByRole("button", { name: "README.md" });
  await user.click(readme);
  const preview = await screen.findByRole("textbox", { name: "文件预览" });
  expect((preview as HTMLTextAreaElement).readOnly).toBe(true);
  await user.click(screen.getByRole("button", { name: "编辑" }));
  const editor = await screen.findByRole("textbox", { name: "文件内容" });
  await user.clear(editor);
  await user.type(editor, "Updated from UI integration test");
  await user.click(screen.getByRole("button", { name: "保存更改" }));
  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "保存更改" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true),
  );
  await user.click(screen.getByRole("button", { name: "操作记录" }));
  await screen.findByRole("columnheader", { name: "操作" });
  expect(screen.getAllByText("写入文件").length).toBeGreaterThan(0);
  await user.click(screen.getByRole("button", { name: "MCP 服务" }));
  await screen.findByText("客户端配置");
  expect(screen.getByText("服务器权限")).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "启动服务" }));
  await screen.findByText(/MCP 服务需要在 Tauri 桌面应用中启动/);
}, 20000);

test("dialogs restore focus, authentication tabs support keyboard, and menus expose actions", async () => {
  const user = userEvent.setup();
  render(<App />);
  await screen.findByText("生产服务器");
  const add = screen.getByRole("button", { name: "添加服务器" });
  await user.click(add);
  const dialog = screen.getByRole("dialog", { name: "添加服务器" });
  expect(dialog).toBeTruthy();
  const keyTab = within(dialog).getByRole("tab", { name: "SSH 私钥" });
  keyTab.focus();
  await user.keyboard("{ArrowRight}");
  expect(
    within(dialog)
      .getByRole("tab", { name: "账户密码" })
      .getAttribute("data-state"),
  ).toBe("active");
  expect(within(dialog).getByLabelText("登录密码")).toBeTruthy();
  const permission = within(dialog).getByRole("checkbox", { name: "写入文本" });
  await user.click(permission);
  expect(permission.getAttribute("aria-checked")).toBe("true");
  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(add));
  await user.click(screen.getByRole("button", { name: "更多操作 生产服务器" }));
  await screen.findByRole("menuitem", { name: "核验指纹" });
  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  const filter = screen.getByRole("combobox", { name: "环境筛选" });
  await user.click(filter);
  await user.click(await screen.findByRole("option", { name: "测试环境" }));
  await screen.findByText("测试服务器");
  expect(screen.queryByText("生产服务器")).toBeNull();
}, 20000);

test("audit dialog has bounded size and shows saved output or an explicit legacy fallback", async () => {
  const user = userEvent.setup();
  render(<App />);
  await screen.findByText("生产服务器");
  await user.click(screen.getByRole("button", { name: "操作记录" }));
  const count = screen.getByRole("status", { name: "记录条目数" });
  await user.click(screen.getByRole("tab", { name: "命令" }));
  expect(count.textContent).toBe(
    `${document.querySelectorAll(".audit-table tbody tr").length.toLocaleString()} 条`,
  );
  const auditSearch = screen.getByRole("textbox", { name: "搜索操作记录" });
  await user.type(auditSearch, "no-matching-audit");
  expect(count.textContent).toBe("0 条");
  await user.clear(auditSearch);
  await user.click(screen.getByRole("tab", { name: "全部" }));
  const row = screen
    .getByText('docker ps --format "table {{.Names}}\\t{{.Status}}"')
    .closest("tr")!;
  await user.click(row);
  const dialog = await screen.findByRole("dialog", { name: "操作详情" });
  expect(dialog.style.width).toBe("620px");
  expect(dialog.style.maxWidth).toBe("calc(100vw - 48px)");
  expect(dialog.style.position).toBe("fixed");
  expect(dialog.style.left).toBe("50%");
  await within(dialog).findByText(/Up 2 hours/);
  expect(within(dialog).getByText("退出码 0")).toBeTruthy();
  expect(within(dialog).getByText("标准输出")).toBeTruthy();
  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  const old = screen.getByText("/etc/nginx/nginx.conf").closest("tr")!;
  await user.click(old);
  await screen.findByText("这条旧记录未保存返回内容。");
}, 15000);

test("Radix preferences persist and enforce the upload limit", async () => {
  localStorage.clear();
  const user = userEvent.setup();
  const view = render(<App />);
  async function choose(label: string, option: string) {
    await user.click(screen.getByRole("combobox", { name: label }));
    await user.click(screen.getByRole("option", { name: option }));
  }
  await user.click(screen.getByRole("button", { name: "设置" }));
  expect(screen.getAllByRole("combobox")).toHaveLength(13);
  expect(
    screen.getByRole("combobox", { name: "终端配色" }).textContent,
  ).toContain("深色");
  expect(document.querySelectorAll("select:not([aria-hidden])")).toHaveLength(
    0,
  );
  await waitFor(() =>
    expect(
      (
        screen.getByRole("spinbutton", {
          name: "单文件上传上限",
        }) as HTMLInputElement
      ).disabled,
    ).toBe(false),
  );
  const uploadLimit = screen.getByRole("spinbutton", {
    name: "单文件上传上限",
  });
  await user.clear(uploadLimit);
  await user.type(uploadLimit, "4");
  await user.tab();
  await waitFor(() =>
    expect(
      (
        screen.getByRole("spinbutton", {
          name: "单文件上传上限",
        }) as HTMLInputElement
      ).disabled,
    ).toBe(false),
  );
  const downloadLimit = screen.getByRole("spinbutton", {
    name: "单文件下载上限",
  });
  await user.clear(downloadLimit);
  await user.type(downloadLimit, "237");
  await user.keyboard("{Enter}");
  await waitFor(() =>
    expect((downloadLimit as HTMLInputElement).disabled).toBe(false),
  );
  await choose("关闭窗口时", "直接退出");
  await waitFor(() =>
    expect(
      screen.getByRole("combobox", { name: "关闭窗口时" }).textContent,
    ).toContain("直接退出"),
  );
  await choose("主题外观", "深色");
  expect(document.documentElement.dataset.theme).toBe("dark");
  await choose("界面密度", "紧凑");
  expect(document.documentElement.dataset.density).toBe("compact");
  await user.click(screen.getByRole("switch", { name: "显示隐藏文件" }));
  await choose("启动页面", "设置");
  await choose("终端字号", "16 px");
  await choose("终端配色", "跟随界面");
  await choose("状态刷新频率", "仅手动刷新");
  view.unmount();
  render(<App />);
  await waitFor(() =>
    expect(
      screen.getByRole("combobox", { name: "关闭窗口时" }).textContent,
    ).toContain("直接退出"),
  );
  await waitFor(() =>
    expect(
      (
        screen.getByRole("spinbutton", {
          name: "单文件上传上限",
        }) as HTMLInputElement
      ).value,
    ).toBe("4"),
  );
  expect(
    (
      screen.getByRole("spinbutton", {
        name: "单文件下载上限",
      }) as HTMLInputElement
    ).value,
  ).toBe("237");
  expect(
    screen.getByRole("combobox", { name: "终端字号" }).textContent,
  ).toContain("16 px");
  expect(
    screen.getByRole("combobox", { name: "终端配色" }).textContent,
  ).toContain("跟随界面");
  expect(
    screen.getByRole("combobox", { name: "状态刷新频率" }).textContent,
  ).toContain("仅手动刷新");
  expect(document.documentElement.dataset.theme).toBe("dark");
  expect(
    screen
      .getByRole("switch", { name: "显示隐藏文件" })
      .getAttribute("aria-checked"),
  ).toBe("false");
  await user.click(screen.getByRole("button", { name: "恢复界面默认设置" }));
  expect(
    screen.getByRole("combobox", { name: "终端字号" }).textContent,
  ).toContain("13 px");
  expect(document.documentElement.dataset.density).toBe("comfortable");
  await user.click(screen.getByRole("button", { name: "远程文件" }));
  await user.click(screen.getByRole("combobox", { name: "选择服务器" }));
  await user.click(screen.getByRole("option", { name: "生产服务器" }));
  await screen.findByRole("button", { name: "README.md" });
  await user.upload(
    document.querySelector('input[type="file"]') as HTMLInputElement,
    new File([new Uint8Array(4 * 1024 * 1024 + 1)], "oversized.bin"),
  );
  await screen.findByText("文件超过 4 MiB 上传限制");
  localStorage.clear();
}, 20000);

test("page actions and terminal actions share the integrated titlebar", async () => {
  localStorage.clear();
  const user = userEvent.setup();
  render(<App />);
  await screen.findByText("生产服务器");
  expect(
    screen.getByRole("button", { name: "添加服务器" }).closest(".window-bar"),
  ).toBeTruthy();
  expect(
    screen.getByRole("button", { name: "最大化" }).closest(".window-bar"),
  ).toBeTruthy();
  expect(document.querySelector(".sidebar-brand")).toBeTruthy();
  expect(document.querySelector(".main-shell .page-heading")).toBeNull();
  await user.click(screen.getByRole("button", { name: "终端会话" }));
  const create = await screen.findByRole("button", { name: "新建终端" });
  expect(create.closest(".window-bar")).toBeTruthy();
  expect(screen.getAllByRole("heading", { name: "终端会话" })).toHaveLength(1);
  expect(screen.getAllByRole("button", { name: "新建终端" })).toHaveLength(1);
  await user.click(screen.getByRole("button", { name: "服务器" }));
  expect(
    screen.getByRole("button", { name: "添加服务器" }).closest(".window-bar"),
  ).toBeTruthy();
  expect(screen.queryByRole("button", { name: "新建终端" })).toBeNull();
});
