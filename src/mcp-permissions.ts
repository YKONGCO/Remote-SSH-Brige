import type { Server } from "./types";

export const permissionGroups = [
  {
    id: "read",
    title: "文件读取",
    description: "列出目录、查询文件属性、读取文本",
    operations: ["read_file", "list_files", "stat_file"],
  },
  {
    id: "write",
    title: "文件写入",
    description: "创建、覆盖和局部编辑文本，自动包含文件读取",
    operations: ["write_file", "edit_file"],
  },
  {
    id: "upload",
    title: "文件上传",
    description: "从本地授权目录上传文件",
    operations: ["upload_file"],
  },
  {
    id: "download",
    title: "文件下载",
    description: "下载文件到本地授权目录",
    operations: ["download_file"],
  },
  {
    id: "exec",
    title: "命令执行",
    description: "执行命令和管理后台任务",
    operations: ["exec", "job"],
  },
  {
    id: "terminal",
    title: "终端操作",
    description: "列出、打开、读取、输入、调整尺寸和关闭终端",
    operations: [
      "list_terminals",
      "terminal_open",
      "terminal_read",
      "terminal_write",
      "terminal_resize",
      "terminal_close",
    ],
  },
  {
    id: "port_forward",
    title: "端口转发",
    description: "创建、查看和关闭 SSH 本地 TCP 转发",
    operations: [
      "port_forward_open",
      "port_forward_list",
      "port_forward_read",
      "port_forward_close",
    ],
  },
  {
    id: "delete",
    title: "文件删除",
    description: "删除远程文件或目录",
    operations: ["remove_path"],
  },
] as const;
export const allMcpPermissions: string[] = permissionGroups.flatMap((group) => [
  ...group.operations,
]);
export function permissionGroupState(
  permissions: string[],
  group: (typeof permissionGroups)[number],
): boolean | "indeterminate" {
  const count = group.operations.filter((kind) =>
    permissions.includes(kind),
  ).length;
  return count === group.operations.length
    ? true
    : count
      ? "indeterminate"
      : false;
}
export function togglePermissionGroup(
  server: Server,
  group: (typeof permissionGroups)[number],
  checked: boolean,
): Server {
  const next = new Set(selectedMcpPermissions(server));
  for (const kind of group.operations) {
    if (checked) next.add(kind);
    else next.delete(kind);
  }
  if (group.id === "write" && checked) {
    next.add("read_file");
    next.add("list_files");
    next.add("stat_file");
  }
  if (group.id === "read" && !checked) {
    next.delete("write_file");
    next.delete("edit_file");
  }
  return withMcpPermissions(server, [...next]);
}

const writePermissionKinds = ["write_file", "edit_file", "remove_path"];
const terminalPermissionKinds = [
  "list_terminals",
  "terminal_open",
  "terminal_read",
  "terminal_write",
  "terminal_resize",
  "terminal_close",
];

// Absent permissions belong to the legacy three-switch configuration.
export function selectedMcpPermissions(server: Server): string[] {
  if (server.mcp_permissions != null)
    return server.mcp_permissions.filter((permission) =>
      allMcpPermissions.some((kind) => kind === permission),
    );
  return [
    "read_file",
    "list_files",
    "stat_file",
    ...(server.allow_write ? ["write_file"] : []),
    ...(server.allow_exec ? ["exec"] : []),
    ...(server.allow_terminal ? terminalPermissionKinds : []),
  ];
}

export function withMcpPermissions(
  server: Server,
  permissions: string[],
): Server {
  permissions = permissions.filter((kind) => kind !== "transfer");
  if (
    permissions.includes("upload_file") ||
    permissions.includes("download_file")
  )
    permissions = [...permissions, "transfer"];
  if (!permissions.includes("read_file"))
    permissions = permissions.filter((kind) => kind !== "edit_file");
  return {
    ...server,
    mcp_permissions: permissions,
    allow_exec: permissions.includes("exec"),
    allow_write: writePermissionKinds.some((kind) =>
      permissions.includes(kind),
    ),
    allow_terminal: terminalPermissionKinds.some((kind) =>
      permissions.includes(kind),
    ),
  };
}

export function mcpPermissionSummary(server: Server): string {
  if (!server.enabled) return "已停用";
  if (server.mcp_enabled === false) return "MCP 已禁用";
  const permissions = selectedMcpPermissions(server);
  const count = permissionGroups.filter(
    (group) => permissionGroupState(permissions, group) !== false,
  ).length;
  const partial = permissionGroups.some(
    (group) => permissionGroupState(permissions, group) === "indeterminate",
  );
  return count
    ? `已授权 ${count} / ${permissionGroups.length} 项${partial ? "（含部分授权）" : ""}`
    : "未授权操作";
}
