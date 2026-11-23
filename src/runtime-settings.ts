export const MAX_TRANSFER_MIB = 8_589_934_591;
export const RUNTIME_SETTINGS_CHANGED = "runtime-settings-changed";
export const SETTINGS_VERSION = 4;
export interface CommandPolicy {
  protected_commands: string[];
  allow_pipes: boolean;
  allow_redirection: boolean;
  allow_compound: boolean;
  allow_scripts: boolean;
  allow_substitution: boolean;
  exec_disabled: boolean;
  legacy_whitelist?: string[] | null;
}
export const MAX_COMMAND_RULES = 4096;
export function parseCommandList(value: string): string[] {
  return [...new Set(value.split(/[\s,;，；、]+/u).filter(Boolean))];
}
function validRule(name: unknown) {
  return (
    typeof name === "string" &&
    name.length > 0 &&
    name.length <= 512 &&
    (name === ":" || /^[\p{L}\p{N}_.+\-/]+$/u.test(name)) &&
    (!name.includes("/") ||
      (name.startsWith("/") &&
        name
          .split("/")
          .slice(1)
          .every((part) => !!part && part !== "." && part !== "..")))
  );
}
export function validCommandRules(rules: unknown) {
  return (
    Array.isArray(rules) &&
    rules.length <= MAX_COMMAND_RULES &&
    rules.every(validRule)
  );
}
export function advancedShell(policy: CommandPolicy) {
  return (
    policy.allow_pipes &&
    policy.allow_redirection &&
    policy.allow_compound &&
    policy.allow_scripts &&
    policy.allow_substitution
  );
}
export const defaultCommandPolicy: CommandPolicy = {
  protected_commands: [],
  allow_pipes: true,
  allow_redirection: true,
  allow_compound: true,
  allow_scripts: true,
  allow_substitution: true,
  exec_disabled: false,
  legacy_whitelist: null,
};
export function effectiveCommandPolicy(
  settings: RuntimeSettings,
): CommandPolicy {
  return { ...defaultCommandPolicy, ...settings.command_policy };
}
export interface AdvancedMcpTools {
  terminal: boolean;
  port_forward: boolean;
}
export const defaultAdvancedMcpTools: AdvancedMcpTools = {
  terminal: false,
  port_forward: true,
};
export interface RuntimeSettings {
  settings_version?: number;
  close_to_tray: boolean;
  mcp_auto_start: boolean;
  mcp_host: string;
  mcp_port: number;
  deletion_protection: boolean;
  command_policy?: CommandPolicy | null;
  remote_read_only: boolean;
  approval_mode: boolean;
  approval_timeout_secs: number;
  upload_limit_mib: number;
  download_limit_mib: number;
  local_upload_roots: string[];
  local_download_roots: string[];
  connect_timeout_secs: number;
  io_timeout_secs: number;
  handshake_timeout_secs: number;
  transfers: TransferSettings;
  command_timeout_secs: number;
  exec_timeout_to_job: boolean;
  max_terminals: number;
  advanced_mcp_tools: AdvancedMcpTools;
}
export interface TransferSettings {
  max_concurrent: number;
  per_server_concurrent: number;
  max_retries: number;
  retry_base_secs: number;
  io_timeout_secs: number;
  idle_timeout_secs: number;
  bandwidth_kib: number;
}

export const defaultTransferSettings: TransferSettings = {
  max_concurrent: 4,
  per_server_concurrent: 2,
  max_retries: 5,
  retry_base_secs: 1,
  io_timeout_secs: 30,
  idle_timeout_secs: 120,
  bandwidth_kib: 0,
};
export const defaultRuntimeSettings: RuntimeSettings = {
  settings_version: SETTINGS_VERSION,
  close_to_tray: true,
  mcp_auto_start: true,
  mcp_host: "127.0.0.1",
  mcp_port: 18920,
  deletion_protection: true,
  remote_read_only: false,
  approval_mode: true,
  approval_timeout_secs: 120,
  upload_limit_mib: 0,
  download_limit_mib: 0,
  local_upload_roots: [],
  local_download_roots: [],
  connect_timeout_secs: 8,
  io_timeout_secs: 15,
  handshake_timeout_secs: 15,
  transfers: { ...defaultTransferSettings },
  command_timeout_secs: 30,
  exec_timeout_to_job: false,
  max_terminals: 32,
  advanced_mcp_tools: { ...defaultAdvancedMcpTools },
};
export type NumericRuntimeSetting = {
  [K in keyof RuntimeSettings]-?: RuntimeSettings[K] extends number ? K : never;
}[keyof RuntimeSettings];

export interface BuiltinRule {
  command: string;
  arguments?: string[];
  reason: string;
}
