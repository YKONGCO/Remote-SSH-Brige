import {
  defaultRuntimeSettings,
  defaultCommandPolicy,
  MAX_TRANSFER_MIB,
  type RuntimeSettings,
} from "./runtime-settings";
import { defaultAllowedCommands, validCommandLists } from "./runtime-settings";
import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  Snapshot,
  Server,
  Operation,
  FileEntry,
  AuditQuery,
  LoginPathUpdate,
} from "./types";
import { chooseBrowserSave } from "./save-file";
import { newServer } from "./types";
import { clientNames } from "./mcp-clients";
import type { JobInfo } from "./jobs";
import type { TunnelInfo } from "./tunnels";
const demoTunnels: TunnelInfo[] = [];
export const desktop = isTauri();
let demoSettings = { ...defaultRuntimeSettings };
const now = Date.now();
const demoJobs: JobInfo[] = [
  {
    job_id: "job_demo_running",
    server_id: "demo-1",
    server_name: "生产服务器",
    command: "tail -f /var/log/app.log",
    source: "mcp",
    created_at: new Date(now - 60000).toISOString(),
    status: "running",
    exit_code: null,
    error: null,
    duration_ms: 60000,
    stdout_end_cursor: 44,
    stderr_end_cursor: 0,
  },
  {
    job_id: "job_demo_exited",
    server_id: "demo-2",
    server_name: "开发服务器",
    command: "sleep 40 && echo completed",
    source: "mcp",
    created_at: new Date(now - 180000).toISOString(),
    status: "exited",
    exit_code: 0,
    error: null,
    duration_ms: 40000,
    stdout_end_cursor: 10,
    stderr_end_cursor: 0,
  },
];
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
    "# Remote SSH Brige\n\n通过 MCP 为 AI Agent 提供可控的 SSH 远程服务器访问。这是浏览器演示文件，实际 SSH 操作请使用桌面应用。\n",
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
  if (command === "list_port_forwards")
    return { tunnels: demoTunnels.map((t) => ({ ...t })) } as T;
  if (command === "port_forward") {
    const request = args.args as {
      action: string;
      server_id: string;
      tunnel_id?: string;
      local_port?: number;
      remote_port?: number;
      remote_host?: string;
    };
    const server = demo.servers.find((s) => s.id === request.server_id);
    if (!server) throw new Error("服务器不存在");
    if (request.action === "open") {
      if (demoSettings.remote_read_only)
        throw new Error("只读模式禁止创建端口转发");
      if (!server.enabled) throw new Error("服务器已停用");
      if (
        !request.remote_port ||
        !Number.isInteger(request.remote_port) ||
        request.remote_port < 1 ||
        request.remote_port > 65535 ||
        !Number.isInteger(request.local_port ?? 0) ||
        (request.local_port ?? 0) < 0 ||
        (request.local_port ?? 0) > 65535
      )
        throw new Error("端口无效");
      if (!["127.0.0.1", "::1"].includes(request.remote_host ?? "127.0.0.1"))
        throw new Error("远端地址无效");
      let port = request.local_port || 18080;
      if (!request.local_port)
        while (
          demoTunnels.some(
            (t) => t.local_port === port && t.status === "running",
          )
        )
          port++;
      if (
        demoTunnels.some((t) => t.local_port === port && t.status === "running")
      )
        throw new Error("端口已被占用");
      const item: TunnelInfo = {
        tunnel_id: crypto.randomUUID(),
        server_id: server.id,
        server_name: server.name,
        source: "desktop",
        local_host: "127.0.0.1",
        local_port: port,
        remote_host: request.remote_host || "127.0.0.1",
        remote_port: request.remote_port,
        status: "running",
        created_at: new Date().toISOString(),
        connections: 0,
        bytes_sent: 0,
        bytes_received: 0,
        error: null,
      };
      demoTunnels.unshift(item);
      return { ...item } as T;
    }
    const item = demoTunnels.find(
      (t) => t.tunnel_id === request.tunnel_id && t.server_id === server.id,
    );
    if (!item) throw new Error("转发不存在");
    if (request.action === "close") item.status = "closed";
    return { ...item } as T;
  }
  if (command === "list_jobs")
    return {
      jobs: demoJobs.map((job) => ({
        ...job,
        duration_ms:
          job.status === "running"
            ? Date.now() - Date.parse(job.created_at)
            : job.duration_ms,
      })),
    } as T;
  if (command === "job") {
    const job = demoJobs.find(
      (item) => item.job_id === args.jobId && item.server_id === args.serverId,
    );
    if (!job) throw new Error("Job 不存在或已过期");
    if (args.action === "close" && job.status === "running") {
      job.status = "closed";
      job.duration_ms = Date.now() - Date.parse(job.created_at);
    }
    const stdout = new TextEncoder().encode(
      job.job_id === "job_demo_exited"
        ? "completed\n"
        : "Worker started.\nWaiting for output...\n",
    );
    return {
      job_id: job.job_id,
      server_id: job.server_id,
      status: job.status,
      exit_code: job.exit_code,
      error: null,
      stdout: new TextDecoder().decode(
        stdout.slice(Number(args.stdoutCursor || 0)),
      ),
      stderr: "",
      next_stdout_cursor: stdout.length,
      next_stderr_cursor: 0,
      stdout_truncated: false,
      stderr_truncated: false,
      stdout_has_more: false,
      stderr_has_more: false,
    } as T;
  }
  if (command === "mcp_clients")
    return Object.entries(clientNames).map(([id, name]) => ({
      id,
      name,
      path: "桌面应用中检测本机配置路径",
      status: "missing",
    })) as T;
  if (command === "configure_mcp_client")
    throw new Error("一键配置需要在桌面应用中使用。");
  if (command === "transfer") return { transfers: [] } as T;
  if (command === "upload_file")
    throw new Error("File streaming requires the desktop app.");
  if (command === "pick_local_directories")
    throw new Error("请选择桌面应用中的文件夹，浏览器预览支持手动填写目录。");
  if (command === "runtime_settings") return structuredClone(demoSettings) as T;
  if (command === "pending_approvals") return [] as T;
  if (command === "resolve_approval")
    throw new Error("操作审批需要在桌面应用中使用。");
  if (command === "save_runtime_settings") {
    const next = args.settings as RuntimeSettings;
    const ranges = {
      approval_timeout_secs: [1, 120],
      upload_limit_mib: [0, MAX_TRANSFER_MIB],
      download_limit_mib: [0, MAX_TRANSFER_MIB],
      connect_timeout_secs: [1, 120],
      io_timeout_secs: [1, 300],
      handshake_timeout_secs: [1, 120],
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
        (key) =>
          !(key in defaultCommandPolicy) &&
          ![
            "execution_mode",
            "allowed_commands",
            "blocked_commands",
            "allow_advanced_shell",
          ].includes(key),
      ) ||
        Object.keys(defaultCommandPolicy).some(
          (key) =>
            typeof next.command_policy![
              key as keyof typeof defaultCommandPolicy
            ] !== "boolean",
        ))
    )
      throw new Error("命令权限设置无效");
    if (
      next.command_policy &&
      ((next.command_policy.execution_mode !== undefined &&
        !["disabled", "whitelist", "unrestricted"].includes(
          next.command_policy.execution_mode,
        )) ||
        (next.command_policy.allow_advanced_shell !== undefined &&
          typeof next.command_policy.allow_advanced_shell !== "boolean") ||
        !validCommandLists(
          next.command_policy.allowed_commands ?? defaultAllowedCommands,
          next.command_policy.blocked_commands ?? [],
        ))
    )
      throw new Error("命令名单或执行范围无效");
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
  if (command === "snapshot")
    return structuredClone({ ...demo, audits: [] }) as T;
  if (command === "audit_page" || command === "export_audits") {
    const query = args.query as AuditQuery;
    const upper = query.upper ?? demo.audits.length;
    const entries = demo.audits
      .slice(0, upper)
      .reverse()
      .filter(
        (a) =>
          (query.category === "all" ||
            (query.category === "mcp" && a.source === "mcp") ||
            (query.category === "error" &&
              (!a.success || a.kind === "security_reject")) ||
            (query.category === "exec" && a.kind === "exec") ||
            (query.category === "files" &&
              [
                "list_files",
                "read_file",
                "write_file",
                "edit_file",
                "remove_path",
                "mkdir",
                "remove",
                "rename",
                "upload",
                "download",
                "upload_file",
                "download_file",
              ].includes(a.kind))) &&
          (!query.start_time ||
            Date.parse(a.time) >= Date.parse(query.start_time)) &&
          (!query.end_time ||
            Date.parse(a.time) < Date.parse(query.end_time)) &&
          (a.server_name + " " + a.detail + " " + a.kind)
            .toLowerCase()
            .includes(query.search.toLowerCase()),
      );
    if (command === "export_audits") {
      const save = await chooseBrowserSave("操作记录.json");
      if (!save) return false as T;
      await save(
        new Blob([JSON.stringify(entries, null, 2)], {
          type: "application/json",
        }),
      );
      return true as T;
    }
    return {
      entries: entries
        .slice(query.offset, query.offset + query.limit)
        .map(({ response: _response, ...entry }) => entry),
      total: entries.length,
      upper,
    } as T;
  }
  if (command === "audit_details") {
    const entry = demo.audits.find((entry) => entry.id === args.id);
    if (!entry) throw new Error("记录不存在");
    return structuredClone(entry) as T;
  }
  if (command === "save_server") {
    const s = { ...(args.server as Server) };
    if (!s.id) s.id = crypto.randomUUID();
    const i = demo.servers.findIndex((v) => v.id === s.id);
    const old = demo.servers[i];
    const same =
      old &&
      old.host === s.host &&
      old.port === s.port &&
      old.username === s.username;
    s.login_path = same ? old.login_path : null;
    s.login_path_synced_at = same ? old.login_path_synced_at : null;
    const pathUpdate = args.loginPathUpdate as LoginPathUpdate | undefined;
    if (pathUpdate && (!old || same)) {
      if (
        pathUpdate.value != null &&
        (pathUpdate.value.includes("\0") ||
          new TextEncoder().encode(pathUpdate.value).length > 65536)
      )
        throw new Error("PATH 不能包含 NUL 或超过 65536 字节");
      s.login_path = pathUpdate.value;
      s.login_path_synced_at = null;
    }
    if (i < 0) demo.servers.push(s);
    else demo.servers[i] = s;
    return s as T;
  }
  if (command === "remove_server") {
    demo.servers = demo.servers.filter((s) => s.id !== args.id);
    return undefined as T;
  }
  if (command === "sync_login_path")
    throw new Error("同步登录 PATH 需要在桌面应用中连接远端服务器。");
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
    "edit_file",
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
  if (
    demoSettings.deletion_protection &&
    ["remove", "remove_path"].includes(op.kind)
  )
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
        stdout: op.cursor
          ? ""
          : "\r\nRemote SSH Brige - browser preview\r\nPermission-controlled SSH access for AI agents over MCP.\r\nUse the desktop app for a real SSH shell.\r\n\r\n$ ",
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
          op.kind === "read_file"
            ? { message: "读取完成，文件正文不写入操作记录" }
            : op.kind === "download"
              ? { message: "下载完成" }
              : (result as Record<string, unknown>),
        truncated: false,
      },
    });
  return result as T;
}
