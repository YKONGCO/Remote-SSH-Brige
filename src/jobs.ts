export type JobStatus = "running" | "exited" | "closed" | "disconnected";
export interface JobInfo {
  job_id: string;
  server_id: string;
  server_name: string;
  command: string;
  source: string;
  created_at: string;
  status: JobStatus;
  exit_code: number | null;
  error: string | null;
  duration_ms: number;
  stdout_end_cursor: number;
  stderr_end_cursor: number;
}
export interface JobOutput {
  status: JobStatus;
  exit_code: number | null;
  error: string | null;
  stdout: string;
  stderr: string;
  next_stdout_cursor: number;
  next_stderr_cursor: number;
  stdout_truncated: boolean;
  stderr_truncated: boolean;
  stdout_has_more: boolean;
  stderr_has_more: boolean;
}
export const jobStatusLabels: Record<JobStatus, string> = {
  running: "运行中",
  exited: "已退出",
  closed: "已关闭",
  disconnected: "已断线",
};
