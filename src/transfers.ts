export interface FileTransfer {
  transfer_id: string;
  server_id: string;
  direction: "upload" | "download";
  local_path: string;
  remote_path: string;
  status: "queued" | "running" | "completed" | "failed" | "cancelled";
  phase: string;
  transferred_bytes: number;
  total_bytes: number | null;
  bytes_per_second: number;
  elapsed_ms: number;
  version: number;
  error: { code: string; message: string } | null;
  audit_error?: string | null;
}
export function formatBytes(bytes: number) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
  let unit = 0;
  while (bytes >= 1024 && unit < units.length - 1) {
    bytes /= 1024;
    unit++;
  }
  return `${unit === 0 ? bytes : bytes.toFixed(1)} ${units[unit]}`;
}
