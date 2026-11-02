import { useEffect, useRef, useState } from "react";
import { Terminal as XTerminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { operate, desktop } from "../api";
import { usePreferences } from "../preferences";
import { terminalTheme } from "../terminal-theme";
import type { Session } from "../types";
import { Button } from "./ui/button";

export type TerminalState =
  "connecting" | "connected" | "retrying" | "ended" | "error";
const labels: Record<TerminalState, string> = {
  connecting: "正在同步…",
  connected: "已连接",
  retrying: "正在重试…",
  ended: "连接已结束",
  error: "读取失败",
};

/** Keep an xterm instance and its cursor mounted while another tab is active. */
export function TerminalSession({
  session,
  active,
  enabled,
  appearance,
  refresh,
  notify,
  onStateChange,
  onNew,
}: {
  session: Session;
  active: boolean;
  enabled: boolean;
  appearance: string;
  refresh: () => Promise<void>;
  notify: (message: string, error?: boolean) => void;
  onStateChange: (id: string, state: TerminalState) => void;
  onNew: () => void;
}) {
  const preferences = usePreferences();
  const host = useRef<HTMLDivElement>(null);
  const terminal = useRef<XTerminal | null>(null);
  const remoteAlive = useRef(true);
  const controls = useRef<{
    wake: () => void;
    fit: () => void;
    retry: () => void;
  } | null>(null);
  const latest = useRef({
    active,
    enabled,
    session,
    refresh,
    notify,
    onStateChange,
  });
  latest.current = { active, enabled, session, refresh, notify, onStateChange };
  const [state, setState] = useState<TerminalState>("connecting");
  const [error, setError] = useState("");

  useEffect(() => {
    if (!host.current) return;
    const term = new XTerminal({
      fontSize: Number(preferences.terminalFontSize),
      fontWeight: appearance === "dark" ? 360 : 400,
      fontWeightBold: appearance === "dark" ? 560 : 600,
      fontFamily: '"Cascadia Code", "Cascadia Mono", Consolas, monospace',
      cursorBlink: preferences.cursorBlink,
      scrollback: Number(preferences.terminalScrollback),
      disableStdin: true,
      theme: terminalTheme(host.current),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);
    terminal.current = term;
    remoteAlive.current = true;
    let disposed = false,
      reading = false,
      writing = false,
      ended = false;
    let failures = 0,
      cursor = 0,
      pending = "",
      pendingBytes = 0,
      lastSize = "";
    let currentState: TerminalState = "connecting";
    let pollTimer: ReturnType<typeof setTimeout> | undefined;
    let inputTimer: ReturnType<typeof setTimeout> | undefined;
    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const encoder = new TextEncoder();
    const update = (next: TerminalState, message = "") => {
      if (disposed) return;
      currentState = next;
      term.options.disableStdin =
        next !== "connected" ||
        !latest.current.active ||
        !latest.current.enabled ||
        !latest.current.session.alive ||
        !remoteAlive.current;
      setState(next);
      setError(message);
      latest.current.onStateChange(session.id, next);
    };
    const schedule = (delay?: number) => {
      clearTimeout(pollTimer);
      if (!disposed && !ended && currentState !== "error") {
        pollTimer = setTimeout(
          () => void poll(),
          delay ?? (latest.current.active && !document.hidden ? 180 : 1500),
        );
      }
    };
    const poll = async () => {
      if (reading || disposed || ended) return;
      reading = true;
      try {
        const result = await operate<{
          stdout: string;
          has_more?: boolean;
          cursor: number;
          alive: boolean;
          truncated: boolean;
        }>({
          kind: "terminal_read",
          server_id: session.server_id,
          session_id: session.id,
          cursor,
        });
        if (disposed) return;
        failures = 0;
        remoteAlive.current = result.alive;
        if (result.truncated) term.write("\r\n[较早输出已被缓冲区清理]\r\n");
        if (result.stdout) {
          // Wait for xterm to consume the output before reading another page.
          await new Promise<void>((resolve) =>
            term.write(result.stdout, resolve),
          );
          if (disposed) return;
        }
        cursor = result.cursor;
        if (!result.alive && !result.has_more) {
          ended = true;
          pending = "";
          pendingBytes = 0;
          term.write("\r\n[终端连接已结束]\r\n");
          update("ended");
          void latest.current.refresh();
        } else {
          update("connected");
        }
      } catch (e) {
        if (disposed) return;
        failures++;
        update(failures < 3 ? "retrying" : "error", String(e));
        if (failures === 3) void latest.current.refresh();
      } finally {
        reading = false;
        schedule(failures ? Math.min(3000, failures * 750) : undefined);
      }
    };
    const flush = async () => {
      if (writing || disposed || ended || !pending) return;
      if (
        currentState !== "connected" ||
        !latest.current.enabled ||
        !latest.current.session.alive ||
        !remoteAlive.current
      ) {
        pending = "";
        pendingBytes = 0;
        return;
      }
      writing = true;
      let count = Math.min(4096, pending.length);
      // Do not split a surrogate pair; each chunk is at most 16 KiB of UTF-8.
      if (count < pending.length && /[\uD800-\uDBFF]/.test(pending[count - 1]))
        count--;
      const content = pending.slice(0, count);
      pending = pending.slice(count);
      pendingBytes -= encoder.encode(content).length;
      try {
        await operate({
          kind: "terminal_write",
          server_id: session.server_id,
          session_id: session.id,
          content,
        });
      } catch (e) {
        pending = "";
        pendingBytes = 0;
        if (!disposed)
          latest.current.notify(
            `终端输入发送失败，未自动重发：${String(e)}`,
            true,
          );
      } finally {
        writing = false;
        if (!disposed && pending)
          inputTimer = setTimeout(() => void flush(), 0);
      }
    };
    const input = term.onData((content) => {
      if (
        !latest.current.active ||
        currentState !== "connected" ||
        !latest.current.enabled ||
        !latest.current.session.alive ||
        !remoteAlive.current
      )
        return;
      if (!desktop) {
        term.write(content === "\r" ? "\r\n$ " : content);
        return;
      }
      const bytes = encoder.encode(content).length;
      if (pendingBytes + bytes > 1024 * 1024) {
        latest.current.notify(
          "待发送输入超过 1 MiB，请等待发送完成后分段粘贴",
          true,
        );
        return;
      }
      pending += content;
      pendingBytes += bytes;
      if (!writing) {
        clearTimeout(inputTimer);
        inputTimer = setTimeout(() => void flush(), 16);
      }
    });
    const refit = () => {
      clearTimeout(resizeTimer);
      if (!latest.current.active || disposed || !host.current?.clientWidth)
        return;
      resizeTimer = setTimeout(() => {
        if (disposed || !latest.current.active) return;
        const buffer = term.buffer.active;
        const scroll = buffer.viewportY;
        const atBottom = scroll >= buffer.baseY;
        fit.fit();
        if (!atBottom) term.scrollToLine(scroll);
        const size = `${term.cols}x${term.rows}`;
        if (
          desktop &&
          currentState === "connected" &&
          latest.current.session.alive &&
          latest.current.enabled &&
          size !== lastSize
        ) {
          lastSize = size;
          void operate({
            kind: "terminal_resize",
            server_id: session.server_id,
            session_id: session.id,
            cols: term.cols,
            rows: term.rows,
          }).catch(() => {
            lastSize = "";
          });
        }
      }, 80);
    };
    const wake = () => {
      clearTimeout(pollTimer);
      if (!ended && currentState !== "error") void poll();
    };
    controls.current = {
      wake,
      fit: refit,
      retry: () => {
        if (!ended) {
          failures = 0;
          update("connecting");
          wake();
        }
      },
    };
    const observer = new ResizeObserver(refit);
    observer.observe(host.current);
    document.addEventListener("visibilitychange", wake);
    window.addEventListener("resize", refit);
    update("connecting");
    void poll();
    refit();
    return () => {
      disposed = true;
      pending = "";
      clearTimeout(pollTimer);
      clearTimeout(inputTimer);
      clearTimeout(resizeTimer);
      observer.disconnect();
      input.dispose();
      document.removeEventListener("visibilitychange", wake);
      window.removeEventListener("resize", refit);
      controls.current = null;
      terminal.current = null;
      term.dispose();
    };
  }, [session.id]);

  useEffect(() => {
    if (!terminal.current) return;
    terminal.current.options.theme = terminalTheme(host.current || undefined);
    terminal.current.options.fontWeight = appearance === "dark" ? 360 : 400;
    terminal.current.options.fontWeightBold = appearance === "dark" ? 560 : 600;
    terminal.current.options.fontSize = Number(preferences.terminalFontSize);
    terminal.current.options.scrollback = Number(
      preferences.terminalScrollback,
    );
    terminal.current.options.cursorBlink = preferences.cursorBlink;
    controls.current?.fit();
  }, [
    appearance,
    preferences.terminalFontSize,
    preferences.terminalScrollback,
    preferences.cursorBlink,
  ]);
  useEffect(() => {
    if (terminal.current)
      terminal.current.options.disableStdin =
        !active ||
        !enabled ||
        !session.alive ||
        !remoteAlive.current ||
        state !== "connected";
  }, [active, enabled, session.alive, state]);
  useEffect(() => {
    if (active) {
      controls.current?.fit();
      if (!document.activeElement?.matches('[role="tab"]:focus-visible'))
        terminal.current?.focus();
      controls.current?.wake();
    }
  }, [active, session.alive]);
  useEffect(() => {
    if (active && state === "connected") controls.current?.fit();
  }, [active, state]);

  return (
    <div
      className="terminal-session"
      hidden={!active}
      role="tabpanel"
      id={`terminal-output-${session.id}`}
      aria-labelledby={`terminal-tab-${session.id}`}
    >
      <div className="terminal-host" ref={host} />
      <div className="terminal-session-status" role="status">
        <span
          className={`status-dot ${state === "connected" ? "green" : "gray"}`}
        />
        <span>{labels[state]}</span>
        {error && (
          <span className="terminal-session-error" title={error}>
            {error}
          </span>
        )}
        {state === "error" && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => controls.current?.retry()}
          >
            重试读取
          </Button>
        )}
        {(state === "ended" || state === "error") && (
          <Button size="sm" variant="ghost" disabled={!enabled} onClick={onNew}>
            新建连接
          </Button>
        )}
      </div>
    </div>
  );
}
