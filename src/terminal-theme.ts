import type { ITheme } from "@xterm/xterm";

/** xterm draws to canvas, so resolve the shared CSS tokens into its theme. */
export function terminalTheme(
  element: HTMLElement = document.documentElement,
): ITheme {
  const css = getComputedStyle(element);
  const root = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string) =>
    css.getPropertyValue(`--terminal-${name}`).trim() ||
    root.getPropertyValue(`--terminal-${name}`).trim() ||
    fallback;
  const foreground = read("foreground", "#18181b");
  const background = read("background", "#f5f5f7");
  return {
    background,
    foreground,
    cursor: foreground,
    cursorAccent: background,
    selectionBackground: read("selection", "#e4e4ea"),
    black: "#18181b",
    brightBlack: "#65656e",
    red: read("red", "#b91c1c"),
    brightRed: read("red", "#b91c1c"),
    green: read("green", "#007634"),
    brightGreen: read("green", "#007634"),
    yellow: read("yellow", "#a64a00"),
    brightYellow: read("yellow", "#a64a00"),
    blue: read("blue", "#0063c7"),
    brightBlue: read("blue", "#0063c7"),
    magenta: read("magenta", "#6d28d9"),
    brightMagenta: read("magenta", "#6d28d9"),
    cyan: read("cyan", "#06726b"),
    brightCyan: read("cyan", "#06726b"),
    white: "#e4e4e7",
    brightWhite: "#f4f4f5",
  };
}
