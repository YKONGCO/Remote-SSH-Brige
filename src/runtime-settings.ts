export const MAX_TRANSFER_MIB = 256;
export interface CommandPolicy {
  diagnostic_only: boolean;
  allow_pipes: boolean;
  allow_redirection: boolean;
  allow_compound: boolean;
  allow_scripts: boolean;
  allow_substitution: boolean;
  allow_mcp_terminal: boolean;
}
export const defaultCommandPolicy: CommandPolicy = {
  diagnostic_only: true,
  allow_pipes: false,
  allow_redirection: false,
  allow_compound: false,
  allow_scripts: false,
  allow_substitution: false,
  allow_mcp_terminal: false,
};
export function effectiveCommandPolicy(
  settings: RuntimeSettings,
): CommandPolicy {
  if (settings.command_policy)
    return { ...defaultCommandPolicy, ...settings.command_policy };
  if (settings.block_dangerous_commands) return { ...defaultCommandPolicy };
  return {
    diagnostic_only: false,
    allow_pipes: true,
    allow_redirection: true,
    allow_compound: true,
    allow_scripts: true,
    allow_substitution: true,
    allow_mcp_terminal: true,
  };
}
export interface RuntimeSettings {
  close_to_tray: boolean;
  deletion_protection: boolean;
  block_dangerous_commands: boolean;
  command_policy?: CommandPolicy | null;
  remote_read_only: boolean;
  approval_mode: "off" | "mcp" | "all";
  upload_limit_mib: number;
  download_limit_mib: number;
  connect_timeout_secs: number;
  io_timeout_secs: number;
  command_timeout_secs: number;
  max_terminals: number;
}
export const defaultRuntimeSettings: RuntimeSettings = {
  close_to_tray: true,
  deletion_protection: true,
  block_dangerous_commands: true,
  remote_read_only: false,
  approval_mode: "off",
  upload_limit_mib: 128,
  download_limit_mib: 128,
  connect_timeout_secs: 8,
  io_timeout_secs: 15,
  command_timeout_secs: 30,
  max_terminals: 32,
};
export type NumericRuntimeSetting = {
  [K in keyof RuntimeSettings]-?: RuntimeSettings[K] extends number ? K : never;
}[keyof RuntimeSettings];
