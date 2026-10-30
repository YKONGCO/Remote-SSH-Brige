import {
  defaultRuntimeSettings,
  defaultCommandPolicy,
  MAX_TRANSFER_MIB,
  type RuntimeSettings,
} from "./runtime-settings";
import { invoke, isTauri } from "@tauri-apps/api/core";
import type { Snapshot, Server, Operation, FileEntry } from "./types";
import { newServer } from "./types";
import { clientNames } from "./mcp-clients";
export const desktop = isTauri();
let demoSettings = { ...defaultRuntimeSettings };
const now = Date.now();
let demo: Snapshot = {
  servers: [
    {
      ...newServer(),
      id: "demo-1",
      name: "生产服务器",
      host: "192.168.1.100",
      group: "生产环境",
      color: "orange",
      username: "ubuntu",
      key_path: "~/.ssh/id_ed25519",
      fingerprint: "SHA256:demo",
      allow_exec: true,
      allow_write: false,
      allow_terminal: true,
      mcp_permissions: null,
    },
    {
      ...newServer(),
      id: "demo-2",
      name: "开发服务器",
      host: "192.168.1.101",
      group: "开发环境",
      color: "blue",
      username: "developer",
      key_path: "~/.ssh/id_ed25519",
      fingerprint: "SHA256:demo",
      allow_exec: true,
      allow_write: true,
      allow_terminal: true,
      mcp_permissions: null,
    },
    {
      ...newServer(),
      id: "demo-3",
      name: "数据库服务器",
      host: "10.0.0.12",
      group: "生产环境",
      color: "purple",
      username: "admin",
      auth: "password",
      fingerprint: "SHA256:demo",
    },
    {
      ...newServer(),
      id: "demo-4",
      name: "测试服务器",
      host: "10.0.0.25",
      group: "测试环境",
      color: "green",
      username: "root",
      key_path: "~/.ssh/id_ed25519",
      fingerprint: "SHA256:demo",
      enabled: false,
    },
  ],
  audits: [
    ["read_file", "/etc/nginx/nginx.conf", true, 128],
    ["exec", 'docker ps --format "table {{.Names}}\\t{{.Status}}"', true, 243],
    ["write_file", "/home/developer/app/.env.example", true, 86],
    ["terminal_open", "交互式 shell", true, 176],
  ].map((r, i) => ({
    id: "audit-" + i,
    time: new Date(now - (i + 1) * 180000).toISOString(),
    server_id: i === 2 ? "demo-2" : "demo-1",
    server_name: i === 2 ? "开发服务器" : "生产服务器",
    source: i === 1 ? "desktop" : "mcp",
    kind: String(r[0]),
    detail: String(r[1]),
    success: Boolean(r[2]),
    duration_ms: Number(r[3]),
    summary: "完成",
    response:
      r[0] === "exec"
        ? {
            data: {
              stdout: "NAMES        STATUS\nweb          Up 2 hours\n",
              stderr: "",
              exit_code: 0,
            },
            truncated: false,
          }
        : undefined,
  })),
  terminals: [],
  mcp: { running: false, port: 18920, token: "" },
  data_dir: "浏览器演示 · 数据仅保存在当前页面内存",
};
let files: Record<string, string | null> = {
  "/etc": null,
  "/home": null,
  "/var": null,
  "/tmp": null,
  "/README.md":
    "# SSH MCP Bridge\n\n通过 MCP 为 AI Agent 提供可控的 SSH 远程服务器访问。这是浏览器演示文件，实际 SSH 操作请使用桌面应用。\n",
  "/etc/nginx": null,
  "/etc/nginx/nginx.conf":
    "worker_processes auto;\nevents { worker_connections 1024; }\nhttp {\n    server { listen 80; root /var/www/html; }\n}\n",
  "/home/developer": null,
  "/home/developer/app": null,
  "/home/developer/app/package.json":
    '{\n  "name": "my-app",\n  "version": "1.0.0"\n}\n',
};
export async function api<T = unknown>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (desktop) return invoke<T>(command, args);
  if (command === "mcp_clients")
    return Object.entries(clientNames).map(([id, name]) => ({
      id,
      name,
      path: "桌面应用中检测本机配置路径",
      status: "missing",
    })) as T;
  if (command === "configure_mcp_client")
    throw new Error("一键配置需要在桌面应用中使用。");
  if (command === "runtime_settings") return structuredClone(demoSettings) as T;
  if (command === "pending_approvals") return [] as T;
  if (command === "resolve_approval")
    throw new Error("操作审批需要在桌面应用中使用。");
  if (command === "save_runtime_settings") {
    const next = args.settings as RuntimeSettings;
    const ranges = {
      upload_limit_mib: [1, MAX_TRANSFER_MIB],
      download_limit_mib: [1, MAX_TRANSFER_MIB],
      connect_timeout_secs: [1, 120],
      io_timeout_secs: [1, 300],
      command_timeout_secs: [1, 300],
      max_terminals: [1, 64],
    };
    if (typeof next.close_to_tray !== "boolean")
      throw new Error("关闭窗口设置无效");
    if (
      ![
        next.deletion_protection,
        next.block_dangerous_commands,
        next.remote_read_only,
      ].every((value) => typeof value === "boolean") ||
      !["off", "mcp", "all"].includes(next.approval_mode)
    )
      throw new Error("安全设置无效");
    if (
      next.command_policy != null &&
      (Object.keys(next.command_policy).some(
        (key) => !(key in defaultCommandPolicy),
      ) ||
        Object.keys(defaultCommandPolicy).some(
          (key) =>
            typeof next.command_policy![
              key as keyof typeof defaultCommandPolicy
            ] !== "boolean",
        ))
    )
      throw new Error("命令权限设置无效");
    for (const key of Object.keys(ranges) as (keyof typeof ranges)[]) {
      if (
        !Number.isInteger(next[key]) ||
        next[key] < ranges[key][0] ||
        next[key] > ranges[key][1]
      )
        throw new Error("设置值超出范围");
    }
    demoSettings = { ...next };
    return structuredClone(demoSettings) as T;
  }
  if (command === "snapshot") return structuredClone(demo) as T;
  if (command === "audit_details") {
    const entry = demo.audits.find((entry) => entry.id === args.id);
    if (!entry) throw new Error("记录不存在");
    return structuredClone(entry) as T;
  }
  if (command === "save_server") {
    const s = { ...(args.server as Server) };
    if (!s.id) s.id = crypto.randomUUID();
    const i = demo.servers.findIndex((v) => v.id === s.id);
    if (i < 0) demo.servers.push(s);
    else demo.servers[i] = s;
    return s as T;
  }
  if (command === "remove_server") {
    demo.servers = demo.servers.filter((s) => s.id !== args.id);
    return undefined as T;
  }
  if (command === "probe_host")
    return "SHA256:BrowserDemoFingerprintForPreviewOnly" as T;
  if (command === "start_mcp" || command === "stop_mcp")
    throw new Error(
      "MCP 服务需要在 Tauri 桌面应用中启动，浏览器只提供界面演示。",
    );
  throw new Error("未知操作");
}
export async function operate<T = Record<string, unknown>>(
  op: Operation,
): Promise<T> {
  if (desktop) return api<T>("operate", { op });
  const server = demo.servers.find((s) => s.id === op.server_id);
  if (!server?.enabled) throw new Error("服务器已停用或不存在");
  const p = op.path || "/";
  const sensitive = [
    "write_file",
    "upload",
    "mkdir",
    "rename",
    "remove",
    "exec",
    "terminal_open",
    "terminal_write",
  ].includes(op.kind);
  if (demoSettings.remote_read_only && sensitive)
    throw new Error("只读保护已开启");
  if (demoSettings.deletion_protection && op.kind === "remove")
    throw new Error("删除保护已开启，请在设置中关闭后再删除");
  if (demoSettings.approval_mode === "all" && sensitive)
    throw new Error("操作审批需要在桌面应用中使用。");
  // Browser preview never executes Shell; the desktop backend applies the AST policy.
  let result: unknown = {};
  switch (op.kind) {
    case "test":
      result = { connected: true };
      break;
    case "list_files": {
      if (p !== "/" && files[p] !== null) throw new Error("目录不存在");
      const prefix = p === "/" ? "/" : p + "/";
      const entries: FileEntry[] = Object.entries(files)
        .filter(
          ([k]) =>
            k.startsWith(prefix) && !k.slice(prefix.length).includes("/"),
        )
        .map(([k, v]) => ({
          name: k.slice(prefix.length),
          path: k,
          is_dir: v === null,
          size: v?.length || 4096,
          modified: Math.floor(now / 1000),
          permissions: v === null ? "755" : "644",
        }));
      entries.sort(
        (a, b) =>
          Number(b.is_dir) - Number(a.is_dir) || a.name.localeCompare(b.name),
      );
      result = { path: p, entries };
      break;
    }
    case "preview_file":
      if (typeof files[p] !== "string") throw new Error("文件不存在");
      result = {
        kind: "text",
        content: files[p],
        bytes: new TextEncoder().encode(files[p]).length,
        truncated: false,
      };
      break;
    case "read_file":
      if (typeof files[p] !== "string") throw new Error("文件不存在");
      result = { content: files[p] };
      break;
    case "write_file":
      files[p] = op.content || "";
      result = { written: op.content?.length };
      break;
    case "upload":
      files[p] = new TextDecoder().decode(
        Uint8Array.from(atob(op.content || ""), (c) => c.charCodeAt(0)),
      );
      break;
    case "mkdir":
      if (p in files) throw new Error("目录已存在");
      files[p] = null;
      break;
    case "remove":
      if (Object.keys(files).some((k) => k.startsWith(p + "/")))
        throw new Error("目录非空");
      delete files[p];
      break;
    case "rename": {
      const target = op.target!;
      if (target in files) throw new Error("目标已存在");
      for (const k of Object.keys(files))
        if (k === p || k.startsWith(p + "/")) {
          files[target + k.slice(p.length)] = files[k];
          delete files[k];
        }
      break;
    }
    case "download":
      result = {
        base64: btoa(
          String.fromCharCode(...new TextEncoder().encode(files[p] || "")),
        ),
      };
      break;
    case "exec":
      result = {
        stdout:
          "[浏览器演示] " +
          op.command +
          "\n\n此处不执行或校验真实命令；桌面端会按命令权限检查后通过 SSH 执行。\n",
        stderr: "",
        exit_code: 0,
      };
      break;
    case "terminal_open": {
      const id = crypto.randomUUID();
      demo.terminals.push({
        id,
        server_id: server.id,
        name: server.name + " · 终端",
        created_at: new Date().toISOString(),
        alive: true,
      });
      result = { session_id: id };
      break;
    }
    case "terminal_read":
      result = {
        base64: op.cursor
          ? ""
          : btoa(
              "\r\nSSH MCP Bridge - browser preview\r\nPermission-controlled SSH access for AI agents over MCP.\r\nUse the desktop app for a real SSH shell.\r\n\r\n$ ",
            ),
        cursor: 1,
        alive: true,
      };
      break;
    case "terminal_write":
    case "terminal_resize":
      break;
    case "terminal_close":
      demo.terminals = demo.terminals.filter((s) => s.id !== op.session_id);
      break;
    default:
      throw new Error("操作不受支持");
  }
  if (!["terminal_read", "terminal_resize"].includes(op.kind))
    demo.audits.push({
      id: crypto.randomUUID(),
      time: new Date().toISOString(),
      server_id: server.id,
      server_name: server.name,
      source: "desktop",
      kind: op.kind,
      detail: op.command || p,
      success: true,
      duration_ms: 32,
      summary: "演示操作",
      response: {
        data:
          op.kind === "read_file" || op.kind === "preview_file"
            ? { message: "读取完成，文件正文不写入操作记录" }
            : op.kind === "download"
              ? { message: "下载完成" }
              : (result as Record<string, unknown>),
        truncated: false,
      },
    });
  return result as T;
}
