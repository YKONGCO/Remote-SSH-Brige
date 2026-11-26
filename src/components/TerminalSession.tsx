import { useEffect, useRef, useState } from "react";
import { Terminal as XTerminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { operate, desktop } from "../api";
import { usePreferences } from "../preferences";
import { terminalTheme } from "../terminal-theme";
import type { Session, TerminalState } from "../types";
import { Button } from "./ui/button";

type SyncState = TerminalState | "retrying" | "error";
const labels: Record<SyncState, string> = {
  connecting: "正在连接…",
  running: "已连接",
  closing: "正在关闭…",
  closed: "连接已结束",
  disconnected: "连接已断开",
  failed: "连接失败",
  retrying: "正在重试读取…",
  error: "读取失败",
};
const lifecycleStates: TerminalState[] = [
  "connecting",
  "running",
  "closing",
  "closed",
  "disconnected",
  "failed",
];
function isLifecycle(state: SyncState): state is TerminalState {
  return lifecycleStates.includes(state as TerminalState);
}

export function TerminalSession({
  session,
  active,
  focusRequest,
  enabled,
  appearance,
  notify,
  onState,
  onNew,
}: {
  session: Session;
  active: boolean;
  focusRequest?: number;
  enabled: boolean;
  appearance: string;
  notify: (message: string, error?: boolean) => void;
  onState: (id: string, state: TerminalState, error?: string) => void;
  onNew: () => void;
}) {
  const preferences = usePreferences();
  const host = useRef<HTMLDivElement>(null);
  const terminal = useRef<XTerminal | null>(null);
  const remoteAlive = useRef(true);
  const lifecycle = useRef<TerminalState>(
    session.state ?? (session.alive ? "running" : "closed"),
  );
  const lastFocusRequest = useRef(focusRequest ?? 0);
  const controls = useRef<{
    wake: () => void;
    fit: () => void;
    retry: () => void;
  } | null>(null);
  const latest = useRef({
    active,
    enabled,
    session,
    notify,
    onState,
  });
  latest.current = { active, enabled, session, notify, onState };
  const [state, setState] = useState<SyncState>(lifecycle.current);
  const [error, setError] = useState(session.error ?? "");

  useEffect(() => {
    if (!host.current) return;
    const startedAt = performance.now();
    const initStart = performance.now();
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
    const frontendInitMs = performance.now() - initStart;
    terminal.current = term;
    remoteAlive.current = true;
    let disposed = false,
      reading = false,
      writing = false,
      ended = false,
      readyLogged = false;
    let failures = 0,
      cursor = 0,
      pending = "",
      pendingBytes = 0,
      lastSize = "";
    let currentState: SyncState = lifecycle.current;
    let pollTimer: ReturnType<typeof setTimeout> | undefined;
    let inputTimer: ReturnType<typeof setTimeout> | undefined;
    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const encoder = new TextEncoder();
    const syncStdin = () => {
      term.options.disableStdin =
        currentState !== "running" ||
        lifecycle.current !== "running" ||
        !latest.current.active ||
        !latest.current.enabled ||
        !latest.current.session.alive ||
        !remoteAlive.current;
    };
    const update = (next: SyncState, message = "") => {
      if (disposed) return;
      currentState = next;
      if (isLifecycle(next) && lifecycle.current !== next) {
        lifecycle.current = next;
        latest.current.onState(session.id, next, message || undefined);
      }
      setState(next);
      setError(message);
      syncStdin();
      if (next === "running" && !readyLogged) {
        readyLogged = true;
        if (desktop)
          console.debug(
            `terminal ${session.id} frontend_init_ms=${frontendInitMs.toFixed(1)} interactive_ready_ms=${(performance.now() - startedAt).toFixed(1)}`,
          );
      }
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
      let continueImmediately = false;
      try {
        const result = await operate<{
          stdout: string;
          has_more?: boolean;
          cursor: number;
          alive: boolean;
          state?: TerminalState;
          error?: string;
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
        const previous = cursor;
        if (result.stdout) term.write(result.stdout);
        cursor = result.cursor;
        continueImmediately = !!result.has_more && cursor > previous;
        const next =
          result.state ??
          (result.alive ? ("running" as const) : ("closed" as const));
        const terminal =
          next === "closed" || next === "disconnected" || next === "failed";
        if (!terminal) {
          update(next);
        } else if (result.has_more) {
          syncStdin();
        } else {
          ended = true;
          pending = "";
          pendingBytes = 0;
          term.write(
            next === "failed" || next === "disconnected"
              ? `\r\n[${result.error || labels[next]}]\r\n`
              : "\r\n[终端连接已结束]\r\n",
          );
          update(next, result.error);
        }
      } catch (e) {
        if (disposed) return;
        failures++;
        update(failures < 3 ? "retrying" : "error", String(e));
      } finally {
        reading = false;
        schedule(
          failures
            ? Math.min(3000, failures * 750)
            : continueImmediately
              ? 0
              : undefined,
        );
      }
    };
    const flush = async () => {
      if (writing || disposed || ended || !pending) return;
      if (
        lifecycle.current !== "running" ||
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
        wake();
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
        lifecycle.current !== "running" ||
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
        const connecting = lifecycle.current === "connecting";
        if (
          desktop &&
          (lifecycle.current === "running" || connecting) &&
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
          update("retrying");
          wake();
        }
      },
    };
    const observer = new ResizeObserver(refit);
    observer.observe(host.current);
    document.addEventListener("visibilitychange", wake);
    window.addEventListener("resize", refit);
    update(lifecycle.current);
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
        state !== "running" ||
        !active ||
        !enabled ||
        !session.alive ||
        !remoteAlive.current;
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
    if (active && state === "running") controls.current?.fit();
  }, [active, state]);
  useEffect(() => {
    if (focusRequest === undefined) return;
    if (lastFocusRequest.current === focusRequest) return;
    lastFocusRequest.current = focusRequest;
    if (!active) return;
    controls.current?.fit();
    terminal.current?.focus();
    controls.current?.wake();
  }, [focusRequest, active]);

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
          className={`status-dot ${state === "running" ? "green" : "gray"}`}
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
        {(state === "closed" ||
          state === "disconnected" ||
          state === "failed" ||
          state === "error") && (
          <Button size="sm" variant="ghost" disabled={!enabled} onClick={onNew}>
            新建连接
          </Button>
        )}
      </div>
    </div>
  );
}
