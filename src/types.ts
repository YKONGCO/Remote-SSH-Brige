export interface Server {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth: "key" | "password" | "agent";
  key_path: string;
  fingerprint: string;
  group: string;
  color: string;
  enabled: boolean;
  allow_exec: boolean;
  allow_write: boolean;
  allow_terminal: boolean;
  mcp_enabled?: boolean;
  mcp_permissions?: string[] | null;
  root: string;
  login_path?: string | null;
  login_path_synced_at?: string | null;
}
export interface LoginPathUpdate {
  value: string | null;
}
export interface Audit {
  id: string;
  time: string;
  server_id: string;
  server_name: string;
  source: string;
  kind: string;
  detail: string;
  success: boolean;
  duration_ms: number;
  summary: string;
  response?: {
    data: Record<string, unknown>;
    truncated: boolean;
    input?: { data: Record<string, unknown>; truncated: boolean } | null;
  } | null;
}
export interface Session {
  id: string;
  server_id: string;
  name: string;
  created_at: string;
  alive: boolean;
}
export interface AuditQuery {
  category: string;
  search: string;
  start_time?: string;
  end_time?: string;
  offset: number;
  limit: number;
  upper?: number;
}
export interface AuditPage {
  entries: Audit[];
  total: number;
  upper: number;
}
export interface McpStatus {
  running: boolean;
  host?: string;
  port: number;
  token: string;
  error?: string;
}
export interface Snapshot {
  servers: Server[];
  audits: Audit[];
  terminals: Session[];
  mcp: McpStatus;
  data_dir: string;
  database_path?: string;
}
export interface Operation {
  kind: string;
  server_id: string;
  path?: string;
  target?: string;
  content?: string;
  command?: string;
  session_id?: string;
  cursor?: number;
  cols?: number;
  rows?: number;
  timeout_secs?: number;
}
export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  is_symlink?: boolean;
  size: number;
  modified: number;
  permissions: string;
}
export interface FilePreview {
  preview_limit_bytes?: number;
  kind: "text";
  content?: string;
  bytes: number;
  truncated: boolean;
}
export interface ApprovalRequest {
  create_parents?: boolean | null;
  recursive?: boolean | null;
  missing_ok?: boolean | null;
  created_at?: string;
  id: string;
  server_id: string;
  server_name: string;
  host: string;
  username: string;
  source: string;
  kind: string;
  path: string;
  target: string;
  command: string;
  content_preview: string;
  content_bytes: number;
  content_sha256: string;
  content_truncated: boolean;
  expires_at: string;
}
export const newServer = (): Server => ({
  id: "",
  name: "",
  host: "",
  port: 22,
  username: "root",
  auth: "key",
  key_path: "",
  fingerprint: "",
  group: "开发环境",
  color: "blue",
  enabled: true,
  allow_exec: false,
  allow_write: false,
  allow_terminal: false,
  mcp_enabled: true,
  mcp_permissions: ["read_file", "list_files", "stat_file"],
  root: "/",
});
export const labels: Record<string, string> = {
  port_forward_open: "创建端口转发",
  port_forward_list: "列出端口转发",
  port_forward_read: "查看端口转发",
  port_forward_close: "关闭端口转发",
  list_servers: "列出服务器",
  list_terminals: "列出终端",
  security_approve: "批准操作",
  security_reject: "拒绝操作",
  exec: "命令执行",
  job: "后台任务",
  upload_file: "上传文件",
  download_file: "下载文件",
  transfer: "文件传输",
  list_files: "浏览目录",
  stat_file: "查询文件属性",
  read_file: "读取文件",
  write_file: "写入文件",
  edit_file: "精确编辑文件",
  remove_path: "安全删除文件 / 目录",
  mkdir: "创建目录",
  remove: "删除文件",
  rename: "重命名",
  upload: "上传文件",
  download: "下载文件",
  test: "测试连接",
  terminal_open: "创建终端",
  terminal_read: "读取终端输出",
  terminal_resize: "调整终端尺寸",
  terminal_write: "终端输入",
  terminal_close: "关闭终端",
};
