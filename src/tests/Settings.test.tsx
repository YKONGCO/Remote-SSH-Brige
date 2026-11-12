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
import { Settings } from "../components/Settings";
import { SelectField } from "../components/SelectField";
import { api } from "../api";
import {
  defaultRuntimeSettings,
  defaultAllowedCommands,
  commandPresets,
  defaultCommandPolicy,
  parseCommandList,
  MAX_COMMAND_RULES,
  validCommandLists,
} from "../runtime-settings";
import appPackage from "../../package.json";

vi.mock("../api", () => ({ desktop: true, api: vi.fn() }));
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});
afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

test("MCP auto-start is on by default, persists on toggle and rolls back a failed save", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  const control = screen.getByRole("switch", { name: "自动启动 MCP 服务" });
  expect(control.hasAttribute("disabled")).toBe(true);
  await waitFor(() => expect(control.hasAttribute("disabled")).toBe(false));
  expect(control.getAttribute("aria-checked")).toBe("true");
  vi.mocked(api).mockImplementationOnce(
    async (_command, args) => args?.settings as never,
  );
  fireEvent.click(control);
  await waitFor(() => expect(control.hasAttribute("disabled")).toBe(false));
  expect(control.getAttribute("aria-checked")).toBe("false");
  expect(api).toHaveBeenCalledWith("save_runtime_settings", {
    settings: expect.objectContaining({
      mcp_auto_start: false,
      mcp_host: "127.0.0.1",
      mcp_port: 18920,
    }),
  });
  vi.mocked(api).mockRejectedValueOnce(new Error("save failed"));
  fireEvent.click(control);
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain("save failed"),
  );
  expect(control.getAttribute("aria-checked")).toBe("false");
});

test("settings footer shows the application version", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  const version = await screen.findByLabelText(
    `应用版本 ${appPackage.version}`,
  );
  expect(version.textContent).toBe(`v${appPackage.version}`);
});

test("settings render with the cached runtime instead of a loading pass", () => {
  vi.mocked(api).mockResolvedValue({ ...defaultRuntimeSettings });
  render(
    <Settings
      initialRuntime={{
        ...defaultRuntimeSettings,
        deletion_protection: false,
      }}
    />,
  );
  const control = screen.getByRole("switch", { name: "删除保护" });
  expect(control.hasAttribute("disabled")).toBe(false);
  expect(control.getAttribute("aria-checked")).toBe("false");
});

test("settings immediately show changes and roll back a failed save without remounting", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    deletion_protection: true,
  });
  render(<Settings />);
  const control = await screen.findByRole("switch", { name: "删除保护" });
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
  expect(screen.getByRole("switch", { name: "删除保护" })).toBe(control);
  expect(control.closest(".settings-layout")?.getAttribute("aria-busy")).toBe(
    "true",
  );
  await act(async () => fail(new Error("save failed")));
  expect(control.getAttribute("aria-checked")).toBe("true");
  expect(screen.getByRole("switch", { name: "删除保护" })).toBe(control);
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

test("approval wait time can be shortened and persists independently of approval mode", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  const timeout = screen.getByRole("combobox", { name: "审批等待时间" });
  expect(timeout.textContent).toContain("120 秒");
  fireEvent.keyDown(timeout, { key: "ArrowDown" });
  fireEvent.click(await screen.findByRole("option", { name: "5 秒" }));
  await waitFor(() => expect(timeout.textContent).toContain("5 秒"));
  const saved = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.approval_timeout_secs).toBe(5);
  expect(saved.approval_mode).toBe("off");
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

test("command presets merge into existing grants, support search and preserve the blacklist", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    command_policy: {
      ...defaultCommandPolicy,
      execution_mode: "whitelist",
      allowed_commands: ["/opt/custom", "python"],
      blocked_commands: ["pip"],
    },
  });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  fireEvent.click(screen.getByRole("button", { name: "管理名单" }));
  const allowed = screen.getByRole("textbox", {
    name: "命令白名单",
  }) as HTMLTextAreaElement;
  expect(parseCommandList(allowed.value)).toEqual(["/opt/custom", "python"]);
  fireEvent.click(screen.getByRole("button", { name: "全选Python 开发" }));
  const python = commandPresets.find((preset) => preset.id === "python")!;
  expect(parseCommandList(allowed.value)).toEqual([
    "/opt/custom",
    ...python.commands,
  ]);
  expect(
    (screen.getByRole("textbox", { name: "命令黑名单" }) as HTMLTextAreaElement)
      .value,
  ).toBe("pip");
  fireEvent.change(screen.getByRole("textbox", { name: "搜索常用命令" }), {
    target: { value: "torchrun" },
  });
  fireEvent.click(screen.getByRole("button", { name: "添加命令 torchrun" }));
  expect(parseCommandList(allowed.value)).toContain("torchrun");
  expect(
    screen
      .getByRole("button", { name: "取消命令 torchrun" })
      .getAttribute("aria-pressed"),
  ).toBe("true");
  expect(
    (
      screen.getByRole("button", {
        name: "取消命令 torchrun",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "取消命令 torchrun" }));
  expect(parseCommandList(allowed.value)).not.toContain("torchrun");
  fireEvent.click(screen.getByRole("button", { name: "添加命令 torchrun" }));
  expect(
    vi
      .mocked(api)
      .mock.calls.some(([name]) => name === "save_runtime_settings"),
  ).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "全选常用命令" }));
  expect(new Set(parseCommandList(allowed.value))).toEqual(
    new Set(["/opt/custom", "python", ...defaultAllowedCommands]),
  );
  fireEvent.click(screen.getByRole("button", { name: "保存名单" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  const saved = vi
    .mocked(api)
    .mock.calls.filter(([name]) => name === "save_runtime_settings")
    .at(-1)![1]!.settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.command_policy?.blocked_commands).toEqual(["pip"]);
  expect(saved.command_policy?.allowed_commands).toContain("/opt/custom");
  expect(saved.command_policy?.allow_scripts).toBe(false);
});

test("bulk command names are deduplicated and presets cannot exceed the command limit", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  fireEvent.click(screen.getByRole("button", { name: "管理名单" }));
  const allowed = screen.getByRole("textbox", {
    name: "命令白名单",
  }) as HTMLTextAreaElement;
  const many = Array.from(
    { length: MAX_COMMAND_RULES - 6 },
    (_, index) => `custom${index}`,
  ).join("\n");
  fireEvent.change(allowed, { target: { value: many } });
  fireEvent.click(screen.getByRole("button", { name: "全选Python 开发" }));
  expect(screen.getByRole("alert").textContent).toContain(
    String(MAX_COMMAND_RULES),
  );
  expect(allowed.value).toBe(many);
  fireEvent.change(allowed, {
    target: { value: "python3, pip；pytest\nnode npm、python3;uv" },
  });
  fireEvent.click(screen.getByRole("button", { name: "保存名单" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  const saved = vi
    .mocked(api)
    .mock.calls.filter(([name]) => name === "save_runtime_settings")
    .at(-1)![1]!.settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.command_policy?.allowed_commands).toEqual([
    "python3",
    "pip",
    "pytest",
    "node",
    "npm",
    "uv",
  ]);
});

test("both command lists accept the expanded limit and reject overflow", () => {
  const commands = Array.from(
    { length: MAX_COMMAND_RULES },
    (_, index) => `tool${index}`,
  );
  expect(validCommandLists(commands, commands)).toBe(true);
  expect(validCommandLists([...commands, "extra"], [])).toBe(false);
  expect(validCommandLists([], [...commands, "extra"])).toBe(false);
});

test("shell presets and the colon builtin validate without allowing arbitrary colons", () => {
  for (const name of [
    "true",
    "false",
    ":",
    "yes",
    "set",
    "bash",
    "sh",
    "python3",
  ]) {
    expect(defaultAllowedCommands).toContain(name);
  }
  expect(defaultAllowedCommands).toContain("python");
  expect(validCommandLists(defaultAllowedCommands, [":"])).toBe(true);
  expect(parseCommandList("true false : yes set bash sh")).toContain(":");
  expect(validCommandLists(["bad:name"], [])).toBe(false);
  expect(validCommandLists(["/tmp/bad:name"], [])).toBe(false);
});

test("preset selection can be reversed globally, by category and within search results", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    command_policy: {
      ...defaultCommandPolicy,
      allowed_commands: ["/opt/custom", "node", "python"],
      blocked_commands: ["pip"],
    },
  });
  render(<Settings />);
  await act(async () => {});
  fireEvent.click(screen.getByRole("button", { name: "管理名单" }));
  const allowed = screen.getByRole("textbox", {
    name: "命令白名单",
  }) as HTMLTextAreaElement;
  fireEvent.click(screen.getByRole("button", { name: "全选Python 开发" }));
  fireEvent.click(screen.getByRole("button", { name: "取消全选Python 开发" }));
  expect(parseCommandList(allowed.value)).toEqual(["/opt/custom", "node"]);
  fireEvent.click(screen.getByRole("button", { name: "全选常用命令" }));
  expect(parseCommandList(allowed.value)).toHaveLength(
    defaultAllowedCommands.length + 1,
  );
  fireEvent.click(screen.getByRole("button", { name: "取消全选" }));
  expect(parseCommandList(allowed.value)).toEqual(["/opt/custom"]);
  fireEvent.change(allowed, { target: { value: "/opt/custom\nnode\npython" } });
  fireEvent.change(screen.getByRole("textbox", { name: "搜索常用命令" }), {
    target: { value: "python3.1" },
  });
  fireEvent.click(
    screen.getByRole("button", { name: "全选Python 开发搜索结果" }),
  );
  expect(parseCommandList(allowed.value)).toContain("python3.14");
  fireEvent.click(
    screen.getByRole("button", { name: "取消全选Python 开发搜索结果" }),
  );
  expect(parseCommandList(allowed.value)).toEqual([
    "/opt/custom",
    "node",
    "python",
  ]);
  expect(
    (screen.getByRole("textbox", { name: "命令黑名单" }) as HTMLTextAreaElement)
      .value,
  ).toBe("pip");
  expect(
    vi
      .mocked(api)
      .mock.calls.some(([name]) => name === "save_runtime_settings"),
  ).toBe(false);
});

test("language toolsets can be browsed and added without replacing existing commands", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    command_policy: {
      ...defaultCommandPolicy,
      allowed_commands: ["/opt/custom"],
      blocked_commands: ["rustup"],
    },
  });
  render(<Settings />);
  await act(async () => {});
  fireEvent.click(screen.getByRole("button", { name: "管理名单" }));
  for (const id of ["cpp", "java", "go", "rust", "dotnet", "containers"]) {
    const preset = commandPresets.find((group) => group.id === id)!;
    fireEvent.click(screen.getByTitle(preset.label));
    expect(
      screen.getByRole("button", { name: `添加命令 ${preset.commands[0]}` }),
    ).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: `全选${preset.label}` }),
    );
    const names = parseCommandList(
      (
        screen.getByRole("textbox", {
          name: "命令白名单",
        }) as HTMLTextAreaElement
      ).value,
    );
    expect(names).toContain("/opt/custom");
    for (const command of preset.commands) expect(names).toContain(command);
  }
  expect(
    (screen.getByRole("textbox", { name: "命令黑名单" }) as HTMLTextAreaElement)
      .value,
  ).toBe("rustup");
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

test("local transfer directories are edited in a dialog and cancel leaves grants unchanged", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    local_upload_roots: ["D:/uploads"],
    local_download_roots: ["D:/downloads"],
  });
  render(<Settings />);
  await act(async () => {});
  expect(screen.queryByRole("textbox", { name: "本地授权目录" })).toBeNull();
  fireEvent.click(
    screen.getByRole("button", { name: "管理 MCP 本地上传目录" }),
  );
  expect(screen.getByRole("dialog").textContent).toContain("MCP 本地上传目录");
  fireEvent.change(screen.getByRole("textbox", { name: "本地授权目录" }), {
    target: { value: "D:/changed" },
  });
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(
    vi
      .mocked(api)
      .mock.calls.some(([command]) => command === "save_runtime_settings"),
  ).toBe(false);
  fireEvent.click(
    screen.getByRole("button", { name: "管理 MCP 本地下载目录" }),
  );
  const editor = screen.getByRole("textbox", {
    name: "本地授权目录",
  }) as HTMLTextAreaElement;
  expect(editor.value).toBe("D:/downloads");
  fireEvent.change(editor, {
    target: { value: " D:/large-files \nD:/large-files\nD:/archives\n" },
  });
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  fireEvent.click(screen.getByRole("button", { name: "保存目录" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  const saved = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.local_download_roots).toEqual(["D:/large-files", "D:/archives"]);
  expect(saved.local_upload_roots).toEqual(["D:/uploads"]);
});

test("entering zero clears the file limit while keeping it editable and the other direction unchanged", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    upload_limit_mib: 256,
    download_limit_mib: 256,
  });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  const input = screen.getByRole("spinbutton", {
    name: "单文件上传上限",
  }) as HTMLInputElement;
  fireEvent.change(input, { target: { value: "0" } });
  fireEvent.blur(input);
  await waitFor(() => expect(input.disabled).toBe(false));
  expect(input.value).toBe("0");
  const saved = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.upload_limit_mib).toBe(0);
  expect(saved.download_limit_mib).toBe(256);
});

test.each([
  ["上传", "local_upload_roots"],
  ["下载", "local_download_roots"],
] as const)(
  "native folder selection appends and deduplicates %s roots before explicit save",
  async (direction, key) => {
    const settings = { ...defaultRuntimeSettings, [key]: ["D:/files"] };
    vi.mocked(api).mockImplementation(async (command, args) => {
      if (command === "runtime_settings") return settings as never;
      if (command === "pick_local_directories")
        return ["d:\\files\\", "D:\\新的目录", "D:\\新的目录"] as never;
      return args?.settings as never;
    });
    render(<Settings />);
    await act(async () => {});
    fireEvent.click(
      screen.getByRole("button", { name: `管理 MCP 本地${direction}目录` }),
    );
    fireEvent.click(screen.getByRole("button", { name: "选择文件夹" }));
    const editor = screen.getByRole("textbox", {
      name: "本地授权目录",
    }) as HTMLTextAreaElement;
    await waitFor(() => expect(editor.value).toBe("D:/files\nD:\\新的目录"));
    expect(
      vi
        .mocked(api)
        .mock.calls.some(([command]) => command === "save_runtime_settings"),
    ).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "保存目录" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const saved = vi
      .mocked(api)
      .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
      .settings as import("../runtime-settings").RuntimeSettings;
    expect(saved[key]).toEqual(["D:/files", "D:\\新的目录"]);
  },
);

test.each(["cancel", "error"])(
  "folder picker %s preserves the draft and does not save",
  async (result) => {
    vi.mocked(api).mockResolvedValueOnce({
      ...defaultRuntimeSettings,
      local_upload_roots: ["D:/existing"],
    });
    render(<Settings />);
    await act(async () => {});
    fireEvent.click(
      screen.getByRole("button", { name: "管理 MCP 本地上传目录" }),
    );
    if (result === "cancel") vi.mocked(api).mockResolvedValueOnce([]);
    else vi.mocked(api).mockRejectedValueOnce(new Error("选择目录失败"));
    fireEvent.click(screen.getByRole("button", { name: "选择文件夹" }));
    await waitFor(() =>
      expect(
        screen
          .getByRole("button", { name: "选择文件夹" })
          .hasAttribute("disabled"),
      ).toBe(false),
    );
    expect(
      (
        screen.getByRole("textbox", {
          name: "本地授权目录",
        }) as HTMLTextAreaElement
      ).value,
    ).toBe("D:/existing");
    if (result === "error")
      expect(screen.getByRole("alert").textContent).toContain("选择目录失败");
    expect(
      vi
        .mocked(api)
        .mock.calls.some(([command]) => command === "save_runtime_settings"),
    ).toBe(false);
  },
);

test("pending folder selection prevents closing or changing the draft", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  fireEvent.click(
    screen.getByRole("button", { name: "管理 MCP 本地上传目录" }),
  );
  let finish!: (paths: string[]) => void;
  vi.mocked(api).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve as typeof finish;
      }),
  );
  fireEvent.click(screen.getByRole("button", { name: "选择文件夹" }));
  for (const name of ["选择中…", "保存目录", "清空目录", "取消"])
    expect(screen.getByRole("button", { name }).hasAttribute("disabled")).toBe(
      true,
    );
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(screen.getByRole("dialog")).toBeTruthy();
  await act(async () => finish(["D:/selected"]));
  expect(
    (
      screen.getByRole("textbox", {
        name: "本地授权目录",
      }) as HTMLTextAreaElement
    ).value,
  ).toBe("D:/selected");
});

test("folder selections exceeding the 64-root limit leave the draft intact", async () => {
  const roots = Array.from({ length: 64 }, (_, i) => `D:/root-${i}`);
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    local_upload_roots: roots,
  });
  render(<Settings />);
  await act(async () => {});
  fireEvent.click(
    screen.getByRole("button", { name: "管理 MCP 本地上传目录" }),
  );
  vi.mocked(api).mockResolvedValueOnce(["D:/extra"]);
  fireEvent.click(screen.getByRole("button", { name: "选择文件夹" }));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "最多授权 64 个目录",
  );
  expect(
    (
      screen.getByRole("textbox", {
        name: "本地授权目录",
      }) as HTMLTextAreaElement
    ).value,
  ).toBe(roots.join("\n"));
});
