import { getCurrentWebview } from "@tauri-apps/api/webview";

export type NativeDropHandlers = {
  onEnter: (paths: string[]) => void;
  onLeave: () => void;
  onDrop: (paths: string[]) => void;
};

/** Native file drops are only available inside the Tauri webview. */
export function nativeDragDropAvailable() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function listenForNativeDrops(handlers: NativeDropHandlers) {
  return getCurrentWebview().onDragDropEvent((event) => {
    switch (event.payload.type) {
      case "enter":
        handlers.onEnter(event.payload.paths);
        break;
      case "leave":
        handlers.onLeave();
        break;
      case "drop":
        handlers.onDrop(event.payload.paths);
        break;
    }
  });
}
