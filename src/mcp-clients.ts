export type ClientId =
  "claude" | "codex" | "gemini" | "opencode" | "cursor" | "pi";
export interface McpClient {
  id: ClientId;
  name: string;
  path: string;
  status: "missing" | "available" | "configured" | "outdated" | "error";
  error?: string | null;
}
export const clientNames: Record<ClientId, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini CLI",
  opencode: "OpenCode",
  cursor: "Cursor",
  pi: "Pi",
};
export function clientConfig(id: ClientId, port: number, token: string) {
  const url = `http://127.0.0.1:${port}/mcp`;
  const headers = { Authorization: `Bearer ${token}` };
  if (id === "codex")
    return `[mcp_servers.remote-desk]\nurl = ${JSON.stringify(url)}\nenabled = true\nhttp_headers = { Authorization = ${JSON.stringify(headers.Authorization)} }`;
  const entry =
    id === "opencode"
      ? { type: "remote", url, headers, enabled: true, oauth: false }
      : id === "gemini"
        ? { httpUrl: url, headers }
        : id === "pi"
          ? { type: "http", url, headers, enabled: true }
          : id === "claude"
            ? { type: "http", url, headers }
            : { url, headers };
  return JSON.stringify(
    { [id === "opencode" ? "mcp" : "mcpServers"]: { "remote-desk": entry } },
    null,
    2,
  );
}
