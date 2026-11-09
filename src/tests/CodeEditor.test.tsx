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
import { EditorView } from "@codemirror/view";
import { undo } from "@codemirror/commands";
import CodeEditor from "../components/CodeEditor";

beforeAll(() => {
  Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
});
afterEach(cleanup);

const callbacks = () => ({
  onChange: vi.fn(),
  onSave: vi.fn(),
  onStats: vi.fn(),
  onLanguage: vi.fn(),
});

test("large documents remain complete while rendering only visible lines", () => {
  const value = ("x".repeat(1023) + "\n").repeat(20 * 1024);
  const props = {
    ...callbacks(),
    filename: "/large.log",
    value,
    readOnly: true,
    wrap: false,
  };
  const rendered = render(<CodeEditor {...props} />);
  const view = EditorView.findFromDOM(
    screen.getByRole("textbox", { name: "文件预览" }),
  )!;
  expect(view.state.sliceDoc()).toBe(value);
  expect(view.state.doc.lines).toBe(20 * 1024 + 1);
  expect(rendered.container.querySelectorAll(".cm-line").length).toBeLessThan(
    100,
  );
  expect(props.onChange).not.toHaveBeenCalled();
});

test("editing preserves CRLF, and switching mode or wrap retains history and selection", () => {
  const props = {
    ...callbacks(),
    filename: "/config.txt",
    value: "first\r\nsecond\r\n",
    readOnly: false,
    wrap: false,
  };
  const rendered = render(<CodeEditor {...props} />);
  const view = EditorView.findFromDOM(
    screen.getByRole("textbox", { name: "文件内容" }),
  )!;
  act(() =>
    view.dispatch({
      changes: { from: 0, to: 5, insert: "changed" },
      selection: { anchor: 7 },
    }),
  );
  expect(props.onChange).toHaveBeenLastCalledWith("changed\r\nsecond\r\n");
  const updated = { ...props, value: "changed\r\nsecond\r\n" };
  rendered.rerender(<CodeEditor {...updated} readOnly wrap />);
  expect(view.state.readOnly).toBe(true);
  expect(view.state.selection.main.head).toBe(7);
  expect(view.contentDOM.classList.contains("cm-lineWrapping")).toBe(true);
  fireEvent.keyDown(view.contentDOM, { key: "Backspace", keyCode: 8 });
  expect(view.state.sliceDoc()).toBe(updated.value);
  rendered.rerender(<CodeEditor {...updated} />);
  act(() => {
    undo(view);
  });
  expect(view.state.sliceDoc()).toBe(props.value);
});

test("language detection loads real syntax highlighting and external loads do not become undo steps", async () => {
  const props = {
    ...callbacks(),
    filename: "/app.ts",
    value: "const answer: number = 42;",
    readOnly: false,
    wrap: false,
  };
  const rendered = render(<CodeEditor {...props} />);
  const view = EditorView.findFromDOM(
    screen.getByRole("textbox", { name: "文件内容" }),
  )!;
  expect(props.onLanguage).toHaveBeenCalledWith("TypeScript");
  await waitFor(() =>
    expect(
      rendered.container.querySelector(".cm-line span[class]"),
    ).toBeTruthy(),
  );
  rendered.rerender(<CodeEditor {...props} value="const full = true;" />);
  expect(view.state.sliceDoc()).toBe("const full = true;");
  act(() => {
    expect(undo(view)).toBe(false);
  });
});
