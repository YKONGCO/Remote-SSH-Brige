export interface TunnelInfo {
  tunnel_id: string;
  server_id: string;
  server_name: string;
  source: string;
  local_host: string;
  local_port: number;
  remote_host: string;
  remote_port: number;
  status: "running" | "closing" | "closed";
  created_at: string;
  connections: number;
  bytes_sent: number;
  bytes_received: number;
  error: string | null;
}
