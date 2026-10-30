// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
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

test("shell feature toggles persist independently of dangerous command blocking", async () => {
  vi.mocked(api).mockResolvedValueOnce({ ...defaultRuntimeSettings });
  render(<Settings />);
  await act(async () => {});
  vi.mocked(api).mockImplementation(
    async (_command, args) => args?.settings as never,
  );
  const pipes = screen.getByRole("switch", { name: "允许管道" });
  fireEvent.click(pipes);
  await waitFor(() => expect(pipes.hasAttribute("disabled")).toBe(false));
  const firstSave = vi
    .mocked(api)
    .mock.calls.find(([command]) => command === "save_runtime_settings")![1]!
    .settings as import("../runtime-settings").RuntimeSettings;
  expect(firstSave.block_dangerous_commands).toBe(true);
  expect(firstSave.command_policy?.allow_pipes).toBe(true);
  expect(firstSave.command_policy?.allow_redirection).toBe(false);
  fireEvent.click(screen.getByRole("switch", { name: "危险命令拦截" }));
  await act(async () => {});
  expect(pipes.getAttribute("aria-checked")).toBe("true");
  expect(
    screen
      .getByRole("switch", { name: "允许脚本执行" })
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
      .getByRole("switch", { name: "允许脚本执行" })
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
