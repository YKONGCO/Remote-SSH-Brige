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
  response?: { data: Record<string, unknown>; truncated: boolean } | null;
}
export interface Session {
  id: string;
  server_id: string;
  name: string;
  created_at: string;
  alive: boolean;
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
  kind: "text" | "image" | "unsupported";
  content?: string;
  base64?: string;
  mime?: string;
  bytes: number;
  truncated: boolean;
}
export interface ApprovalRequest {
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
  mcp_permissions: ["read_file", "preview_file"],
  root: "/",
});
export const labels: Record<string, string> = {
  preview_file: "预览文件",
  security_approve: "批准操作",
  security_reject: "拒绝操作",
  exec: "命令执行",
  list_files: "浏览目录",
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
  terminal_write: "终端输入",
  terminal_close: "关闭终端",
};
