import { afterEach, beforeAll, expect, test, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { Settings } from "../components/Settings";
import { SelectField } from "../components/SelectField";
import { api } from "../api";
import {
  defaultRuntimeSettings,
  defaultCommandPolicy,
  parseProtectedCommandRules,
  MAX_COMMAND_RULES,
  validCommandRules,
  validProtectedCommandRules,
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
  const mcp = screen.getByRole("heading", { name: "MCP" });
  const security = screen.getByRole("heading", { name: "安全" });
  expect(
    mcp.compareDocumentPosition(security) & Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
  const control = within(mcp.closest("section")!).getByRole("switch", {
    name: "自动启动 MCP 服务",
  });
  expect(
    within(security.closest("section")!).getByRole("switch", {
      name: "允许高危命令审批",
    }),
  ).toBeTruthy();
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

test("settings render cached values immediately and ignore a load superseded by a save", async () => {
  let finishLoad!: (settings: typeof defaultRuntimeSettings) => void;
  vi.mocked(api)
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishLoad = resolve;
        }),
    )
    .mockImplementationOnce(async (_command, args) => args?.settings as never);
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
  fireEvent.click(control);
  await waitFor(() => expect(control.hasAttribute("disabled")).toBe(false));
  await act(async () =>
    finishLoad({ ...defaultRuntimeSettings, deletion_protection: false }),
  );
  expect(control.getAttribute("aria-checked")).toBe("true");
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

test("approval wait time persists for protected command approvals", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  const summary = screen.getByText("高级设置");
  const details = summary.closest("details");
  expect(details?.open).toBe(false);
  details!.open = true;
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
});

test("one advanced shell toggle controls all shell capabilities and leaves advanced MCP tools independent", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  const summary = screen.getByText("高级设置");
  const details = summary.closest("details")!;
  expect(details.open).toBe(false);
  details.open = true;
  expect(
    screen.getByText(
      "允许管道、重定向、组合命令及脚本。命中受保护规则时仍需审批。",
    ),
  ).toBeTruthy();
  const advanced = screen.getByRole("switch", {
    name: "允许 Shell 高级功能",
  });
  expect(advanced.getAttribute("aria-checked")).toBe("true");
  fireEvent.click(advanced);
  await waitFor(() => expect(advanced.hasAttribute("disabled")).toBe(false));
  const firstSave = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  for (const key of [
    "allow_pipes",
    "allow_redirection",
    "allow_compound",
    "allow_scripts",
    "allow_substitution",
  ] as const)
    expect(firstSave.command_policy?.[key]).toBe(false);
  expect(firstSave.advanced_mcp_tools).toEqual({
    terminal: false,
    port_forward: true,
  });
  details.open = false;
  expect(api).toHaveBeenCalledTimes(2);
  const terminal = screen.getByRole("switch", { name: "交互式终端" });
  expect(terminal.getAttribute("aria-checked")).toBe("false");
  fireEvent.click(terminal);
  await waitFor(() =>
    expect(terminal.getAttribute("aria-checked")).toBe("true"),
  );
  const saved = vi
    .mocked(api)
    .mock.calls.filter(([command]) => command === "save_runtime_settings")
    .at(-1)![1]!.settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.advanced_mcp_tools).toEqual({
    terminal: true,
    port_forward: true,
  });
});

test("protected command dialog shows read-only builtin rules and saves custom rules", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(async (command, args) => {
    if (command === "protected_rules")
      return {
        builtin: [
          { command: "rm", reason: "Deletes files or directories" },
          {
            command: "find",
            arguments: ["-delete", "-exec"],
            reason: "find may delete files",
          },
        ],
      } as never;
    return args?.settings as never;
  });
  expect(screen.getAllByRole("button", { name: /受保护命令/ })).toHaveLength(1);
  expect(screen.queryByText("旧版白名单限制")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "管理受保护命令" }));
  expect(await screen.findByText("内置规则（只读，始终启用）")).toBeTruthy();
  expect(await screen.findByText("rm")).toBeTruthy();
  expect(await screen.findByText(/find may delete files/)).toBeTruthy();
  const editor = screen.getByRole("textbox", {
    name: "自定义受保护命令",
  }) as HTMLTextAreaElement;
  fireEvent.change(editor, { target: { value: "docker && rmi\ncurl" } });
  fireEvent.click(screen.getByRole("button", { name: "保存规则" }));
  expect(screen.getByRole("alert").textContent).toContain("Shell 语法");
  expect(
    vi
      .mocked(api)
      .mock.calls.some(([command]) => command === "save_runtime_settings"),
  ).toBe(false);
  fireEvent.change(editor, {
    target: { value: "curl\n/opt/deploy\ndocker rmi" },
  });
  fireEvent.click(screen.getByRole("button", { name: "保存规则" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  const saved = vi
    .mocked(api)
    .mock.calls.filter(([command]) => command === "save_runtime_settings")
    .at(-1)![1]!.settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.command_policy?.protected_commands).toEqual([
    "curl",
    "/opt/deploy",
    "docker rmi",
  ]);
  expect(saved.command_policy?.allow_pipes).toBe(true);
});

test("legacy whitelist restriction stays visible until explicitly cleared", async () => {
  vi.mocked(api).mockResolvedValueOnce({
    ...defaultRuntimeSettings,
    command_policy: {
      ...defaultCommandPolicy,
      protected_commands: ["curl"],
      legacy_whitelist: ["ls", "date"],
    },
  });
  render(<Settings />);
  await act(async () => {});
  expect(screen.getByText("旧版白名单限制")).toBeTruthy();
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "改用新版受保护命令策略" }),
  );
  await waitFor(() =>
    expect(
      vi
        .mocked(api)
        .mock.calls.some(([command]) => command === "save_runtime_settings"),
    ).toBe(true),
  );
  const saved = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.command_policy?.legacy_whitelist).toBeNull();
  expect(screen.queryByText("旧版白名单限制")).toBeNull();
  expect(saved.command_policy?.protected_commands).toEqual(["curl"]);
});

test("approval mode can be disabled without changing command limits", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  const approval = screen.getByRole("switch", { name: "允许高危命令审批" });
  expect(approval.getAttribute("aria-checked")).toBe("true");
  fireEvent.click(approval);
  await waitFor(() =>
    expect(approval.getAttribute("aria-checked")).toBe("false"),
  );
  const saved = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.approval_mode).toBe(false);
  expect(saved.command_policy?.exec_disabled).toBe(false);
});

test("command execution can be disabled without touching advanced shell permissions", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  screen.getByText("高级设置").closest("details")!.open = true;
  fireEvent.click(screen.getByRole("switch", { name: "禁止命令执行" }));
  await act(async () => {});
  const saved = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(saved.command_policy?.exec_disabled).toBe(true);
  expect(saved.command_policy?.allow_pipes).toBe(true);
  expect(saved.advanced_mcp_tools).toEqual({
    terminal: false,
    port_forward: true,
  });
});

test("protected rules accept literal argument prefixes and keep one rule per line", () => {
  expect(validCommandRules(["curl", "/opt/deploy", ":", "python3.14"])).toBe(
    true,
  );
  expect(validCommandRules(["rm -rf /"])).toBe(false);
  expect(
    validProtectedCommandRules([
      "curl",
      "/opt/deploy --target=prod",
      "docker rmi",
    ]),
  ).toBe(true);
  for (const rule of [
    "docker && rmi",
    "docker *",
    "docker\nrmi",
    "docker $action",
  ]) {
    expect(validProtectedCommandRules([rule])).toBe(false);
  }
  expect(validCommandRules(["../escape"])).toBe(false);
  const maximum = Array.from(
    { length: MAX_COMMAND_RULES },
    (_, index) => `tool${index}`,
  );
  expect(validCommandRules(maximum)).toBe(true);
  expect(validCommandRules([...maximum, "extra"])).toBe(false);
  expect(
    parseProtectedCommandRules(" curl\r\n docker\t rmi\n\ndocker rmi"),
  ).toEqual(["curl", "docker rmi"]);
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
