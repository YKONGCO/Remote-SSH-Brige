import { createPortal } from "react-dom";
import { usePreferences } from "../preferences";
import { SelectField } from "./SelectField";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Button } from "./ui/button";
import { useEffect, useRef, useState } from "react";
import {
  Plus,
  TerminalSquare,
  X,
  Maximize2,
  Server as ServerIcon,
} from "lucide-react";
import type {
  Server,
  Session,
  TerminalOpenRequest,
  TerminalState,
} from "../types";
import { TerminalSession } from "./TerminalSession";

function stateOf(session: Session): TerminalState {
  return session.state ?? (session.alive ? "running" : "closed");
}
function liveState(state: TerminalState): boolean {
  return state === "connecting" || state === "running";
}

export function Terminals({
  server,
  servers,
  onServerChange,
  sessions,
  openRequest,
  onOpenRequestConsumed,
  onCreate,
  creatingServers,
  closingIds,
  onClose,
  onTerminalState,
  notify,
  headerHost,
}: {
  headerHost?: HTMLElement | null;
  server?: Server;
  servers: Server[];
  onServerChange: (id: string) => void;
  sessions: Session[];
  openRequest?: TerminalOpenRequest | null;
  onOpenRequestConsumed?: () => void;
  onCreate: (serverId: string) => Promise<string | null>;
  creatingServers: Set<string>;
  closingIds: Set<string>;
  onClose: (session: Session) => Promise<void>;
  onTerminalState: (id: string, state: TerminalState, error?: string) => void;
  notify: (message: string, error?: boolean) => void;
}) {
  const preferences = usePreferences();
  const [appearance, setAppearance] = useState("light");
  const frame = useRef<HTMLDivElement>(null);
  const [activeByServer, setActiveByServer] = useState<Record<string, string>>(
    {},
  );
  const [visited, setVisited] = useState<Set<string>>(() => new Set());
  const [focusRequest, setFocusRequest] = useState(0);
  const [closing, setClosing] = useState<Session | null>(null);
  const [closingId, setClosingId] = useState("");
  const handledRequest = useRef(0);
  const serverIdRef = useRef(server?.id);
  serverIdRef.current = server?.id;
  const removing = useRef(false);
  const list = sessions.filter((s) => !server || s.server_id === server.id);
  const selected =
    list.find((s) => s.id === activeByServer[server?.id || ""]) ||
    list.find((s) => liveState(stateOf(s))) ||
    list[0];
  const busy = server ? creatingServers.has(server.id) : false;
  useEffect(() => {
    if (selected)
      setVisited((previous) =>
        previous.has(selected.id)
          ? previous
          : new Set([...previous, selected.id]),
      );
  }, [selected?.id]);
  useEffect(() => {
    const apply = () =>
      setAppearance(
        preferences.terminalTheme === "app"
          ? document.documentElement.dataset.theme || "light"
          : preferences.terminalTheme,
      );
    apply();
    const observer = new MutationObserver(apply);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => observer.disconnect();
  }, [preferences.terminalTheme, preferences.theme]);
  const select = (session: Session) =>
    setActiveByServer((previous) => ({
      ...previous,
      [session.server_id]: session.id,
    }));
  const createSession = (target: Server) => onCreate(target.id);
  const activateSession = (targetId: string, sessionId: string) => {
    setActiveByServer((previous) => ({
      ...previous,
      [targetId]: sessionId,
    }));
    if (serverIdRef.current === targetId)
      setFocusRequest((previous) => previous + 1);
  };
  const openForServer = async (target: Server) => {
    if (!target.enabled) return;
    const usable = (session: Session) =>
      session.server_id === target.id &&
      session.alive &&
      liveState(stateOf(session));
    const available = list.filter(usable);
    const active = available.find(
      (session) => session.id === activeByServer[target.id],
    );
    const running =
      (active && stateOf(active) === "running" ? active : undefined) ||
      available.find((session) => stateOf(session) === "running");
    const connecting =
      (active && stateOf(active) === "connecting" ? active : undefined) ||
      available.find((session) => stateOf(session) === "connecting");
    const chosen = running || connecting;
    if (chosen) {
      activateSession(target.id, chosen.id);
      return;
    }
    const sessionId = await createSession(target);
    if (sessionId) activateSession(target.id, sessionId);
  };
  const open = async () => {
    if (!server?.enabled) return;
    try {
      const sessionId = await createSession(server);
      if (sessionId) activateSession(server.id, sessionId);
    } catch (e) {
      notify(String(e), true);
    }
  };
  useEffect(() => {
    if (!openRequest || !server || openRequest.serverId !== server.id) return;
    if (handledRequest.current === openRequest.requestId) return;
    handledRequest.current = openRequest.requestId;
    onOpenRequestConsumed?.();
    void openForServer(server).catch((e) => notify(String(e), true));
  }, [openRequest?.requestId, server?.id]);
  const close = async (session: Session) => {
    if (removing.current) return;
    removing.current = true;
    setClosingId(session.id);
    try {
      await onClose(session);
      const siblings = sessions.filter(
        (s) => s.server_id === session.server_id,
      );
      const index = siblings.findIndex((s) => s.id === session.id);
      const next = siblings[index + 1] || siblings[index - 1];
      setActiveByServer((previous) =>
        previous[session.server_id] === session.id ||
        selected?.id === session.id
          ? { ...previous, [session.server_id]: next?.id || "" }
          : previous,
      );
      setClosing(null);
    } catch (e) {
      notify(String(e), true);
    } finally {
      removing.current = false;
      setClosingId("");
    }
  };
  const requestClose = (session: Session) => {
    if (!session.alive || !liveState(stateOf(session))) void close(session);
    else setClosing(session);
  };
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
                      aria-controls={`terminal-output-${session.id}`}
                      id={"terminal-tab-" + session.id}
                      className="terminal-tab"
                      title={`${session.name} · ${
                        stateOf(session) === "connecting"
                          ? "正在连接"
                          : stateOf(session) === "closing"
                            ? "正在关闭"
                            : stateOf(session) === "running"
                              ? "运行中"
                              : "已结束"
                      }`}
                      tabIndex={session.id === selected?.id ? 0 : -1}
                      onClick={() => select(session)}
                      onKeyDown={(event) => {
                        let next: Session | undefined;
                        if (event.key === "ArrowRight")
                          next = list[(index + 1) % list.length];
                        if (event.key === "ArrowLeft")
                          next = list[(index + list.length - 1) % list.length];
                        if (event.key === "Home") next = list[0];
                        if (event.key === "End") next = list[list.length - 1];
                        if (next) {
                          event.preventDefault();
                          select(next);
                          document
                            .getElementById(`terminal-tab-${next.id}`)
                            ?.focus();
                        }
                      }}
                    >
                      <TerminalSquare size={13} />
                      <span className="terminal-tab-label">
                        终端 {index + 1}
                      </span>
                      <span
                        className={
                          "status-dot " +
                          (session.alive && liveState(stateOf(session))
                            ? "green"
                            : "gray")
                        }
                      />
                    </button>
                    <button
                      className="terminal-tab-close"
                      aria-label={`${session.alive && liveState(stateOf(session)) ? "关闭" : "移除"}终端 ${index + 1}`}
                      title={
                        session.alive && liveState(stateOf(session))
                          ? "关闭终端"
                          : "移除已结束会话"
                      }
                      disabled={!!closingId || closingIds.has(session.id)}
                      onClick={() => requestClose(session)}
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
                  const element = frame.current;
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
          {sessions
            .filter(
              (session) =>
                visited.has(session.id) || session.id === selected?.id,
            )
            .map((session) => (
              <TerminalSession
                key={session.id}
                session={session}
                active={session.id === selected?.id}
                focusRequest={focusRequest}
                enabled={
                  servers.find((item) => item.id === session.server_id)
                    ?.enabled ?? false
                }
                appearance={appearance}
                notify={notify}
                onState={onTerminalState}
                onNew={() => void open()}
              />
            ))}
          {!selected && (
            <div className="terminal-empty">
              <TerminalSquare size={29} />
              <h3>尚无终端会话</h3>
            </div>
          )}
        </div>
      </section>
      {closing && (
        <ModalFrame
          onClose={() => {
            if (!closingId) setClosing(null);
          }}
          className="compact-modal"
        >
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
                disabled={!!closingId}
                onClick={() => setClosing(null)}
              >
                取消
              </Button>
              <Button
                variant="destructive"
                size="sm"
                className="btn danger-fill"
                disabled={!!closingId}
                onClick={() => void close(closing)}
              >
                {closingId ? "正在关闭…" : "关闭会话"}
              </Button>
            </div>
          </div>
        </ModalFrame>
      )}
    </>
  );
}
