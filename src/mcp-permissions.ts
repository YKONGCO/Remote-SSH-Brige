import type { Server } from "./types";

export const permissionGroups = [
  {
    title: "文件读取",
    operations: [
      ["read_file", "读取文本"],
      ["preview_file", "预览文件"],
    ],
  },
  {
    title: "文件修改",
    operations: [
      ["write_file", "写入文本"],
      ["edit_file", "精确编辑（需读取权限）"],
      ["remove_path", "安全删除文件 / 目录"],
    ],
  },
  { title: "命令执行", operations: [["exec", "执行命令"]] },
  {
    title: "终端操作",
    operations: [
      ["list_terminals", "列出会话"],
      ["terminal_open", "创建终端"],
      ["terminal_read", "读取输出"],
      ["terminal_write", "发送输入"],
      ["terminal_resize", "调整尺寸"],
      ["terminal_close", "关闭终端"],
    ],
  },
] as const;

export const allMcpPermissions = permissionGroups.flatMap((group) =>
  group.operations.map(([kind]) => kind),
);

// Absent permissions belong to the legacy three-switch configuration.
export function selectedMcpPermissions(server: Server): string[] {
  if (server.mcp_permissions != null)
    return server.mcp_permissions.filter((permission) =>
      allMcpPermissions.some((kind) => kind === permission),
    );
  return permissionGroups.flatMap((group, index) =>
    index === 0 ||
    (index === 1 && server.allow_write) ||
    (index === 2 && server.allow_exec) ||
    (index === 3 && server.allow_terminal)
      ? group.operations
          .filter(([kind]) => kind !== "edit_file" && kind !== "remove_path")
          .map(([kind]) => kind)
      : [],
  );
}

export function withMcpPermissions(
  server: Server,
  permissions: string[],
): Server {
  if (!permissions.includes("read_file"))
    permissions = permissions.filter((kind) => kind !== "edit_file");
  return {
    ...server,
    mcp_permissions: permissions,
    allow_exec: permissions.includes("exec"),
    allow_write: permissionGroups[1].operations.some(([kind]) =>
      permissions.includes(kind),
    ),
    allow_terminal: permissionGroups[3].operations.some(([kind]) =>
      permissions.includes(kind),
    ),
  };
}

export function mcpPermissionSummary(server: Server): string {
  if (!server.enabled) return "已停用";
  if (server.mcp_enabled === false) return "MCP 已禁用";
  const count = allMcpPermissions.filter((kind) =>
    selectedMcpPermissions(server).includes(kind),
  ).length;
  return count
    ? `已授权 ${count} / ${allMcpPermissions.length} 项`
    : "未授权操作";
}
