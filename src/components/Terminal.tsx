import { createPortal } from "react-dom";
import { usePreferences } from "../preferences";
import { SelectField } from "./SelectField";
import { terminalTheme } from "../terminal-theme";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";
import { useEffect, useRef, useState } from "react";
import { Terminal as XTerminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import {
  Plus,
  TerminalSquare,
  X,
  Maximize2,
  RefreshCw,
  Server as ServerIcon,
} from "lucide-react";
import type { Server, Session } from "../types";
import { operate, desktop } from "../api";
export function Terminals({
  server,
  servers,
  onServerChange,
  sessions,
  refresh,
  notify,
  headerHost,
}: {
  headerHost?: HTMLElement | null;
  server?: Server;
  servers: Server[];
  onServerChange: (id: string) => void;
  sessions: Session[];
  refresh: () => Promise<void>;
  notify: (s: string, error?: boolean) => void;
}) {
  const preferences = usePreferences();
  const [appearance, setAppearance] = useState(() =>
    preferences.terminalTheme === "app"
      ? document.documentElement.dataset.theme || "light"
      : preferences.terminalTheme,
  );
  const frame = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState(""),
    [busy, setBusy] = useState(false),
    [closing, setClosing] = useState<Session | null>(null);
  const list = sessions.filter((s) => !server || s.server_id === server.id);
  const selected = list.find((s) => s.id === active) || list[0];
  const host = useRef<HTMLDivElement>(null),
    term = useRef<XTerminal | null>(null);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const apply = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        const next =
          preferences.terminalTheme === "app"
            ? document.documentElement.dataset.theme || "light"
            : preferences.terminalTheme;
        setAppearance(next);
        if (frame.current) frame.current.dataset.terminalTheme = next;
        if (!term.current) return;
        term.current.options.theme = terminalTheme(frame.current || undefined);
        const dark = next === "dark";
        term.current.options.fontWeight = dark ? 360 : 400;
        term.current.options.fontWeightBold = dark ? 560 : 600;
      }, 0);
    };
    apply();
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    media?.addEventListener("change", apply);
    return () => {
      clearTimeout(timer);
      media?.removeEventListener("change", apply);
    };
  }, [preferences.theme, preferences.terminalTheme, selected?.id]);
  useEffect(() => {
    if (!term.current) return;
    term.current.options.fontSize = Number(preferences.terminalFontSize);
    term.current.options.scrollback = Number(preferences.terminalScrollback);
    term.current.options.cursorBlink = preferences.cursorBlink;
    window.dispatchEvent(new Event("resize"));
  }, [
    preferences.terminalFontSize,
    preferences.terminalScrollback,
    preferences.cursorBlink,
  ]);
  const open = async () => {
    if (!server) return;
    setBusy(true);
    try {
      const r = await operate<{ session_id: string }>({
        kind: "terminal_open",
        server_id: server.id,
        cols: 100,
        rows: 28,
      });
      setActive(r.session_id);
      await refresh();
    } catch (e) {
      notify(String(e), true);
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    if (!host.current || !selected) return;
    const terminal = new XTerminal({
      fontSize: Number(preferences.terminalFontSize),
      fontWeight: appearance === "dark" ? 360 : 400,
      fontWeightBold: appearance === "dark" ? 560 : 600,
      fontFamily: '"Cascadia Code", "Cascadia Mono", Consolas, monospace',
      cursorBlink: preferences.cursorBlink,
      scrollback: Number(preferences.terminalScrollback),
      theme: terminalTheme(frame.current || undefined),
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host.current);
    fit.fit();
    term.current = terminal;
    let cursor = 0,
      stopped = false,
      working = false;
    let writeQueue = Promise.resolve();
    const sub = terminal.onData((content) => {
      if (!desktop) {
        terminal.write(content === "\r" ? "\r\n$ " : content);
        return;
      }
      writeQueue = writeQueue
        .then(() =>
          operate({
            kind: "terminal_write",
            server_id: selected.server_id,
            session_id: selected.id,
            content,
          }),
        )
        .then(() => {})
        .catch((e) => notify(String(e), true));
    });
    const poll = async () => {
      if (working || stopped) return;
      working = true;
      try {
        const r = await operate<{
          base64: string;
          cursor: number;
          alive: boolean;
          truncated: boolean;
        }>({
          kind: "terminal_read",
          server_id: selected.server_id,
          session_id: selected.id,
          cursor,
        });
        if (stopped) return;
        if (r.truncated) terminal.write("\r\n[较早输出已被缓冲区清理]\r\n");
        if (r.base64)
          terminal.write(
            Uint8Array.from(atob(r.base64), (c) => c.charCodeAt(0)),
          );
        cursor = r.cursor;
        if (!r.alive) {
          terminal.write("\r\n[终端连接已结束]\r\n");
          stopped = true;
          void refresh();
        }
      } catch (e) {
        if (!stopped) {
          terminal.write("\r\n[" + String(e) + "]\r\n");
          stopped = true;
        }
      } finally {
        working = false;
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), 180);
    let resizeTimer: ReturnType<typeof setTimeout>;
    const refit = () => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        fit.fit();
        if (desktop)
          void operate({
            kind: "terminal_resize",
            server_id: selected.server_id,
            session_id: selected.id,
            cols: terminal.cols,
            rows: terminal.rows,
          }).catch(() => {});
      }, 100);
    };
    const observer = new ResizeObserver(refit);
    window.addEventListener("resize", refit);
    observer.observe(host.current);
    terminal.focus();
    return () => {
      stopped = true;
      clearInterval(timer);
      clearTimeout(resizeTimer);
      observer.disconnect();
      window.removeEventListener("resize", refit);
      sub.dispose();
      terminal.dispose();
      term.current = null;
    };
  }, [selected?.id]);
  const toolbar = (
    <div className="terminal-heading-actions">
      <div className="server-select">
        <ServerIcon size={14} />
        <SelectField
          aria-label="选择服务器"
          value={server?.id || ""}
          onValueChange={onServerChange}
        >
          {servers.length ? (
            servers.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name}
                {!item.enabled ? "（已停用）" : ""}
              </option>
            ))
          ) : (
            <option value="">尚无服务器</option>
          )}
        </SelectField>
      </div>
      <Button
        variant="ghost"
        size="icon"
        className="icon-btn"
        aria-label="刷新会话"
        title="刷新会话"
        onClick={() => void refresh()}
      >
        <RefreshCw size={14} />
      </Button>
      <Button
        size="sm"
        className="btn primary"
        disabled={!server?.enabled || busy}
        onClick={() => void open()}
      >
        <Plus size={14} />
        {busy ? "正在连接…" : "新建终端"}
      </Button>
    </div>
  );
  return (
    <>
      <section className="terminal-workspace" aria-label="终端工作区">
        {headerHost !== undefined ? (
          headerHost ? (
            createPortal(toolbar, headerHost)
          ) : null
        ) : (
          <header className="page-heading terminal-heading">
            <h1>终端会话</h1>
            {toolbar}
          </header>
        )}
        <div
          className="terminal-window"
          ref={frame}
          data-terminal-theme={appearance}
        >
          {list.length > 0 && (
            <div className="terminal-tabs">
              <div
                className="terminal-tab-list"
                role="tablist"
                aria-label="终端会话"
              >
                {list.map((session, index) => (
                  <div
                    className={
                      "terminal-tab-group " +
                      (session.id === selected?.id ? "active" : "")
                    }
                    key={session.id}
                  >
                    <button
                      role="tab"
                      aria-selected={session.id === selected?.id}
                      aria-controls="terminal-output"
                      id={"terminal-tab-" + session.id}
                      className="terminal-tab"
                      title={session.name}
                      onClick={() => setActive(session.id)}
                    >
                      <TerminalSquare size={13} />
                      <span className="terminal-tab-label">
                        终端 {index + 1}
                      </span>
                      <span
                        className={
                          "status-dot " + (session.alive ? "green" : "gray")
                        }
                      />
                    </button>
                    <button
                      className="terminal-tab-close"
                      aria-label={"关闭终端 " + (index + 1)}
                      title="关闭终端"
                      onClick={() => setClosing(session)}
                    >
                      <X size={12} />
                    </button>
                  </div>
                ))}
              </div>
              <button
                className="terminal-expand"
                title="终端全屏"
                aria-label="终端全屏"
                onClick={() => {
                  const element = host.current?.parentElement;
                  if (element) {
                    if (document.fullscreenElement)
                      void document.exitFullscreen();
                    else
                      void element
                        .requestFullscreen()
                        .catch(() => notify("当前窗口不支持全屏", true));
                  }
                }}
              >
                <Maximize2 size={14} />
              </button>
            </div>
          )}
          {selected ? (
            <div
              className="terminal-host"
              id="terminal-output"
              role="tabpanel"
              aria-labelledby={"terminal-tab-" + selected.id}
              ref={host}
            />
          ) : (
            <div className="terminal-empty">
              <TerminalSquare size={29} />
              <h3>尚无终端会话</h3>
            </div>
          )}
        </div>
      </section>
      {closing && (
        <ModalFrame onClose={() => setClosing(null)} className="compact-modal">
          <div className="modal-layout">
            <div className="modal-head">
              <DialogTitle asChild>
                <h2>关闭终端会话？</h2>
              </DialogTitle>
            </div>
            <div className="modal-body">
              <p>
                「{closing.name}」的 SSH 通道将关闭。前台进程可能随会话结束。
              </p>
            </div>
            <div className="modal-foot">
              <Button
                variant="outline"
                size="sm"
                className="btn"
                onClick={() => setClosing(null)}
              >
                取消
              </Button>
              <Button
                variant="destructive"
                size="sm"
                className="btn danger-fill"
                onClick={async () => {
                  try {
                    await operate({
                      kind: "terminal_close",
                      server_id: closing.server_id,
                      session_id: closing.id,
                    });
                    await refresh();
                    setClosing(null);
                  } catch (e) {
                    notify(String(e), true);
                  }
                }}
              >
                关闭会话
              </Button>
            </div>
          </div>
        </ModalFrame>
      )}
    </>
  );
}
