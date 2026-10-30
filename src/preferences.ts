import { useSyncExternalStore } from "react";

export const defaults = {
  theme: "system",
  density: "comfortable",
  motion: true,
  startPage: "servers",
  refreshSeconds: "3",
  showHidden: true,
  terminalFontSize: "13",
  terminalTheme: "dark",
  terminalScrollback: "10000",
  cursorBlink: true,
};
export type Preferences = typeof defaults;
const key = "remote-desk.preferences.v1";
export function readPreferences(): Preferences {
  try {
    const saved = JSON.parse(localStorage.getItem(key) || "{}");
    const result = { ...defaults };
    const choices: Record<string, string[]> = {
      theme: ["system", "light", "dark"],
      density: ["comfortable", "compact"],
      startPage: ["servers", "files", "terminals", "audit", "mcp", "settings"],
      refreshSeconds: ["0", "3", "5", "10", "30"],
      terminalFontSize: ["12", "13", "14", "15", "16"],
      terminalTheme: ["dark", "light", "app"],
      terminalScrollback: ["1000", "5000", "10000", "20000"],
    };
    for (const name of Object.keys(defaults) as (keyof Preferences)[]) {
      if (
        typeof defaults[name] === "boolean"
          ? typeof saved[name] === "boolean"
          : choices[name]?.includes(saved[name])
      )
        Object.assign(result, { [name]: saved[name] });
    }
    return result;
  } catch {
    return { ...defaults };
  }
}
export function usePreferences() {
  const raw = useSyncExternalStore(
    (notify) => {
      window.addEventListener("storage", notify);
      window.addEventListener("preferences-changed", notify);
      return () => {
        window.removeEventListener("storage", notify);
        window.removeEventListener("preferences-changed", notify);
      };
    },
    () => {
      try {
        return localStorage.getItem(key);
      } catch {
        return null;
      }
    },
  );
  void raw;
  return readPreferences();
}
export function savePreferences(value: Preferences) {
  localStorage.setItem(key, JSON.stringify(value));
  window.dispatchEvent(new Event("preferences-changed"));
}
