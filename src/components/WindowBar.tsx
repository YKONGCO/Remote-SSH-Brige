import { useEffect, useState, type ReactNode, type Ref } from "react";
import { Minus, Square, Copy, X, PanelTopClose } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { desktop } from "../api";
export function WindowBar({
  onError,
  title = "SSH MCP Bridge",
  actionsRef,
  children,
}: {
  onError: (message: string) => void;
  title?: string;
  actionsRef?: Ref<HTMLDivElement>;
  children?: ReactNode;
}) {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!desktop) return;
    const win = getCurrentWindow();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const update = () =>
      void win
        .isMaximized()
        .then((v) => {
          if (!disposed) setMaximized(v);
        })
        .catch(() => {});
    update();
    void win.onResized(update).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  const action = async (kind: "minimize" | "maximize" | "hide" | "close") => {
    if (!desktop) return;
    try {
      const win = getCurrentWindow();
      if (kind === "minimize") await win.minimize();
      else if (kind === "maximize") {
        await win.toggleMaximize();
        setMaximized(await win.isMaximized());
      } else if (kind === "close") await win.close();
      else await win.hide();
    } catch (e) {
      onError(String(e));
    }
  };
  return (
    <header className="window-bar">
      <div
        className="window-drag"
        data-tauri-drag-region
        onDoubleClick={() => void action("maximize")}
      >
        <h1 data-tauri-drag-region>{title}</h1>
      </div>
      <div className="titlebar-actions" ref={actionsRef}>
        {children}
      </div>
      <div className="window-controls">
        <button
          title="收起到托盘"
          aria-label="收起到托盘"
          disabled={!desktop}
          onClick={() => void action("hide")}
        >
          <PanelTopClose size={14} />
        </button>
        <button
          title="最小化"
          aria-label="最小化"
          disabled={!desktop}
          onClick={() => void action("minimize")}
        >
          <Minus size={15} />
        </button>
        <button
          title={maximized ? "还原窗口" : "最大化"}
          aria-label={maximized ? "还原窗口" : "最大化"}
          disabled={!desktop}
          onClick={() => void action("maximize")}
        >
          {maximized ? <Copy size={13} /> : <Square size={13} />}
        </button>
        <button
          className="window-close"
          title="关闭窗口"
          aria-label="关闭窗口"
          disabled={!desktop}
          onClick={() => void action("close")}
        >
          <X size={17} />
        </button>
      </div>
    </header>
  );
}
