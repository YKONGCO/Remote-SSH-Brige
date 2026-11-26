import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { desktop } from "./api";

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  date?: string;
  body?: string;
}

export interface UpdateProgress {
  downloaded: number;
  total?: number;
}

let pending: Update | null = null;

export async function checkForUpdate(): Promise<UpdateInfo | null> {
  if (!desktop) throw new Error("自动更新需要在桌面应用中使用。");
  await discardUpdate();
  const update = await check();
  if (!update) return null;
  pending = update;
  return {
    version: update.version,
    currentVersion: update.currentVersion,
    date: update.date,
    body: update.body,
  };
}

export async function installUpdate(
  onProgress: (progress: UpdateProgress) => void,
): Promise<void> {
  if (!pending) throw new Error("请先检查更新。");
  let downloaded = 0;
  let total: number | undefined;
  const update = pending;
  await update.downloadAndInstall((event) => {
    if (event.event === "Started") {
      total = event.data.contentLength;
    } else if (event.event === "Progress") {
      downloaded += event.data.chunkLength;
      onProgress({ downloaded, total });
    }
  });
  pending = null;
  await update.close().catch(() => {});
  await relaunch();
}

export async function discardUpdate(): Promise<void> {
  if (!pending) return;
  const update = pending;
  pending = null;
  await update.close().catch(() => {});
}
