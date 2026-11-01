// @vitest-environment jsdom
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { Settings } from "./Settings";
import { SelectField } from "./SelectField";
import { api } from "../api";
import { defaultRuntimeSettings } from "../runtime-settings";

vi.mock("../api", () => ({ desktop: true, api: vi.fn() }));
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});
afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

test("settings immediately show changes and roll back a failed save without remounting", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    deletion_protection: true,
  });
  render(<Settings />);
  const control = await screen.findByRole("switch", { name: "桌面删除保护" });
  await act(async () => {});
  let fail!: (error: Error) => void;
  vi.mocked(api).mockImplementationOnce(
    () =>
      new Promise((_resolve, reject) => {
        fail = reject;
      }),
  );
  fireEvent.click(control);
  expect(control.getAttribute("aria-checked")).toBe("false");
  expect(screen.getByRole("switch", { name: "桌面删除保护" })).toBe(control);
  expect(control.closest(".settings-layout")?.getAttribute("aria-busy")).toBe(
    "true",
  );
  await act(async () => fail(new Error("save failed")));
  expect(control.getAttribute("aria-checked")).toBe("true");
  expect(screen.getByRole("switch", { name: "桌面删除保护" })).toBe(control);
  expect(screen.getByRole("alert").textContent).toContain("save failed");
});

test("select labels stay visible when values update with the menu closed", () => {
  const options = [
    <option key="a" value="a">
      选项 A
    </option>,
    <option key="b" value="b">
      选项 B
    </option>,
  ];
  const view = render(
    <SelectField value="a" onValueChange={vi.fn()} aria-label="测试选择">
      {options}
    </SelectField>,
  );
  const trigger = screen.getByRole("combobox", { name: "测试选择" });
  expect(trigger.textContent).toContain("选项 A");
  view.rerender(
    <SelectField value="b" onValueChange={vi.fn()} aria-label="测试选择">
      {options}
    </SelectField>,
  );
  expect(screen.getByRole("combobox", { name: "测试选择" })).toBe(trigger);
  expect(trigger.textContent).toContain("选项 B");
});

test("one advanced shell toggle saves all shell capabilities and leaves the AI terminal independent", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  const pipes = screen.getByRole("switch", { name: "允许 Shell 高级功能" });
  fireEvent.click(pipes);
  await waitFor(() => expect(pipes.hasAttribute("disabled")).toBe(false));
  const firstSave = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(firstSave.block_dangerous_commands).toBe(true);
  expect(firstSave.command_policy?.allow_pipes).toBe(true);
  expect(firstSave.command_policy?.allow_redirection).toBe(true);
  expect(firstSave.command_policy?.allow_scripts).toBe(true);
  expect(firstSave.command_policy?.allow_compound).toBe(true);
  expect(firstSave.command_policy?.allow_substitution).toBe(true);
  expect(firstSave.command_policy?.allow_mcp_terminal).toBe(false);
  expect(firstSave.command_policy?.allow_advanced_shell).toBe(true);
  fireEvent.click(screen.getByRole("switch", { name: "危险命令拦截" }));
  await act(async () => {});
  expect(pipes.getAttribute("aria-checked")).toBe("true");
  expect(
    screen
      .getByRole("switch", { name: "允许 AI 交互终端" })
      .getAttribute("aria-checked"),
  ).toBe("false");
});

test("legacy unrestricted settings preserve shell permissions when enabling protection", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    block_dangerous_commands: false,
  });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  expect(
    screen
      .getByRole("switch", { name: "允许 Shell 高级功能" })
      .getAttribute("aria-checked"),
  ).toBe("true");
  fireEvent.click(screen.getByRole("switch", { name: "危险命令拦截" }));
  await act(async () => {});
  const call = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")!;
  const saved = call[1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.block_dangerous_commands).toBe(true);
  expect(saved.command_policy?.allow_scripts).toBe(true);
  expect(saved.command_policy?.diagnostic_only).toBe(false);
});

test("command lists can be edited, cleared and restored, and invalid rules stay unsaved", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  fireEvent.click(screen.getByRole("button", { name: "管理名单" }));
  const allowed = screen.getByRole("textbox", { name: "命令白名单" });
  const blocked = screen.getByRole("textbox", { name: "命令黑名单" });
  fireEvent.change(allowed, { target: { value: "ls\ngrep\n/opt/report\nls" } });
  fireEvent.change(blocked, { target: { value: "rm -rf /" } });
  fireEvent.click(screen.getByRole("button", { name: "保存名单" }));
  expect(screen.getByRole("alert").textContent).toContain("不要填写参数");
  expect(
    vi
      .mocked(api)
      .mock.calls.filter(([command]) => command === "save_runtime_settings"),
  ).toHaveLength(0);
  fireEvent.change(blocked, { target: { value: "grep\nrm" } });
  fireEvent.click(screen.getByRole("button", { name: "保存名单" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  let saved = vi
    .mocked(api)
    .mock.calls.filter(([command]) => command === "save_runtime_settings")
    .at(-1)![1]!.settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.command_policy?.execution_mode).toBe("whitelist");
  expect(saved.command_policy?.allowed_commands).toEqual([
    "ls",
    "grep",
    "/opt/report",
  ]);
  expect(saved.command_policy?.blocked_commands).toEqual(["grep", "rm"]);
  fireEvent.click(screen.getByRole("button", { name: "管理名单" }));
  expect(
    (screen.getByRole("textbox", { name: "命令黑名单" }) as HTMLTextAreaElement)
      .value,
  ).toBe("grep\nrm");
  fireEvent.change(screen.getByRole("textbox", { name: "命令白名单" }), {
    target: { value: "" },
  });
  fireEvent.click(screen.getByRole("button", { name: "保存名单" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  saved = vi
    .mocked(api)
    .mock.calls.filter(([command]) => command === "save_runtime_settings")
    .at(-1)![1]!.settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.command_policy?.allowed_commands).toEqual([]);
  fireEvent.click(screen.getByRole("button", { name: "管理名单" }));
  fireEvent.click(screen.getByRole("button", { name: "恢复默认名单" }));
  expect(
    (screen.getByRole("textbox", { name: "命令白名单" }) as HTMLTextAreaElement)
      .value,
  ).toContain("pwd\nls");
  expect(
    (screen.getByRole("textbox", { name: "命令黑名单" }) as HTMLTextAreaElement)
      .value,
  ).toBe("");
});

test("execution mode changes are persisted without enabling advanced shell or the AI terminal", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  const mode = screen.getByRole("combobox", { name: "命令执行范围" });
  fireEvent.keyDown(mode, { key: "ArrowDown" });
  fireEvent.click(await screen.findByRole("option", { name: "禁止执行" }));
  await waitFor(() => expect(mode.textContent).toContain("禁止执行"));
  const saved = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.command_policy?.execution_mode).toBe("disabled");
  expect(saved.command_policy?.allow_scripts).toBe(false);
  expect(saved.command_policy?.allow_mcp_terminal).toBe(false);
});
