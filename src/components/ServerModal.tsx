import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";
import { EnvironmentGroup } from "./EnvironmentGroup";
import { Checkbox } from "./ui/checkbox";
import { ModalFrame, DialogTitle } from "./ModalFrame";
import { Input } from "./ui/input";
import { Button } from "./ui/button";
import { useEffect, useState } from "react";
import {
  X,
  Server as ServerIcon,
  ShieldCheck,
  KeyRound,
  ArrowRight,
} from "lucide-react";
import type { Server } from "../types";
import {
  permissionGroups,
  selectedMcpPermissions,
  withMcpPermissions,
  permissionGroupState,
  togglePermissionGroup,
} from "../mcp-permissions";

import { api } from "../api";
import type { RuntimeSettings } from "../runtime-settings";

const serverColors = [
  { value: "blue", label: "雾蓝", swatch: "var(--direct-text)" },
  { value: "orange", label: "暖橙", swatch: "var(--warning-text)" },
  { value: "purple", label: "浅紫", swatch: "var(--stack-text)" },
  { value: "green", label: "松绿", swatch: "var(--route-text)" },
  { value: "cyan", label: "湖青", swatch: "var(--chart-1)" },
  { value: "rose", label: "玫红", swatch: "var(--chart-2)" },
  { value: "red", label: "珊瑚红", swatch: "var(--danger-text)" },
  { value: "gray", label: "岩灰", swatch: "var(--text-2)" },
];

export function ServerModal({
  initial,
  groups = [],
  onClose,
  onSave,
  onOpenSettings,
}: {
  onOpenSettings?: () => void;
  initial: Server;
  groups?: string[];
  onClose: () => void;
  onSave: (s: Server, secret: string) => Promise<void>;
}) {
  const [server, setServer] = useState(initial),
    [secret, setSecret] = useState(""),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const set = (key: keyof Server, value: unknown) =>
    setServer((s) => ({ ...s, [key]: value }));
  const [deletionProtection, setDeletionProtection] = useState<boolean | null>(
    null,
  );
  const [settingsError, setSettingsError] = useState("");
  useEffect(() => {
    let active = true;
    api<RuntimeSettings>("runtime_settings")
      .then((settings) => {
        if (active) setDeletionProtection(settings.deletion_protection);
      })
      .catch(() => {
        if (active)
          setSettingsError("无法读取删除保护设置，请关闭窗口后重试。");
      });
    return () => {
      active = false;
    };
  }, []);
  const selectedPermissions = selectedMcpPermissions(server);
  const mcpEnabled = server.mcp_enabled !== false;
  return (
    <ModalFrame
      onClose={() => {
        if (!busy) onClose();
      }}
      className="server-modal"
    >
      <form
        className="modal-form"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          setError("");
          try {
            await onSave(
              withMcpPermissions(server, selectedPermissions),
              secret,
            );
            onClose();
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <div className="modal-head">
          <div className="heading-icon">
            <ServerIcon size={21} />
          </div>
          <div>
            <DialogTitle asChild>
              <h2>{server.id ? "编辑服务器" : "添加服务器"}</h2>
            </DialogTitle>
          </div>
          <Button
            variant="ghost"
            size="icon"
            type="button"
            className="icon-btn"
            onClick={onClose}
            aria-label="关闭"
          >
            <X size={20} />
          </Button>
        </div>
        <div className="modal-body">
          <div className="section-label">基本信息</div>
          <div className="form-grid">
            <label>
              服务器名称
              <Input
                required
                placeholder="例如：生产服务器"
                value={server.name}
                onChange={(e) => set("name", e.target.value)}
              />
            </label>
            <label>
              环境分组
              <EnvironmentGroup
                value={server.group}
                groups={groups}
                onChange={(value) => set("group", value)}
              />
            </label>
            <label>
              主机地址
              <Input
                required
                placeholder="IP 地址或域名"
                value={server.host}
                onChange={(e) => set("host", e.target.value)}
              />
            </label>
            <label>
              SSH 端口
              <Input
                required
                type="number"
                min="1"
                max="65535"
                value={server.port}
                onChange={(e) => set("port", +e.target.value)}
              />
            </label>
            <label>
              标识颜色
              <Select
                value={server.color}
                onValueChange={(value) => set("color", value)}
              >
                <SelectTrigger aria-label="标识颜色">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {serverColors.map((color) => (
                    <SelectItem key={color.value} value={color.value}>
                      <span className="server-color-option">
                        <span
                          className="server-color-swatch"
                          style={{ backgroundColor: color.swatch }}
                          aria-hidden="true"
                        />
                        {color.label}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </label>
          </div>
          <div className="section-label">
            <KeyRound size={14} />
            身份认证
          </div>
          <Tabs
            value={server.auth}
            onValueChange={(value) => {
              set("auth", value);
              setSecret("");
            }}
          >
            <TabsList className="auth-tabs h-10 w-full" aria-label="认证方式">
              <TabsTrigger value="key">SSH 私钥</TabsTrigger>
              <TabsTrigger value="password">账户密码</TabsTrigger>
              <TabsTrigger value="agent">代理认证</TabsTrigger>
            </TabsList>
          </Tabs>
          <label>
            登录账号
            <Input
              required
              name="username"
              autoComplete="username"
              placeholder="请输入服务器登录账号，例如 root 或 ubuntu"
              value={server.username}
              onChange={(e) => set("username", e.target.value)}
            />
          </label>
          {server.auth === "key" && (
            <label>
              本地私钥路径
              <Input
                required
                placeholder="C:\Users\you\.ssh\id_ed25519"
                value={server.key_path}
                onChange={(e) => set("key_path", e.target.value)}
              />
            </label>
          )}
          {server.auth !== "agent" && (
            <label>
              {server.auth === "key" ? "私钥口令（可选）" : "登录密码"}
              <Input
                type="password"
                name="password"
                required={
                  server.auth === "password" &&
                  (!server.id || initial.auth !== "password")
                }
                autoComplete="new-password"
                placeholder={
                  server.id && initial.auth === server.auth
                    ? "留空保留已保存的凭据"
                    : server.auth === "password"
                      ? "请输入服务器登录密码"
                      : "私钥有口令时请填写，无口令可留空"
                }
                value={secret}
                onChange={(e) => setSecret(e.target.value)}
              />
            </label>
          )}
          <div className="section-label">
            <ShieldCheck size={14} />
            MCP 访问权限
          </div>
          <label className="mcp-access-toggle">
            <Checkbox
              checked={mcpEnabled}
              disabled={busy}
              onCheckedChange={(checked) =>
                set("mcp_enabled", checked === true)
              }
            />
            允许 MCP 访问此服务器
          </label>
          <label>
            文件访问根目录
            <Input
              required
              value={server.root}
              onChange={(e) => set("root", e.target.value)}
            />
          </label>
          <div className="mcp-permission-groups">
            <fieldset disabled={!mcpEnabled || busy}>
              <legend>MCP 操作权限</legend>
              <div className="permission-group-content permission-categories">
                <button
                  type="button"
                  className="permission-select-all"
                  disabled={!mcpEnabled || busy}
                  onClick={() =>
                    setServer((current) => {
                      const available = permissionGroups.filter(
                        (group) =>
                          group.id !== "delete" || deletionProtection === false,
                      );
                      const allSelected = available.every(
                        (group) =>
                          permissionGroupState(
                            selectedMcpPermissions(current),
                            group,
                          ) === true,
                      );
                      return available.reduce(
                        (next, group) =>
                          togglePermissionGroup(next, group, !allSelected),
                        current,
                      );
                    })
                  }
                >
                  {permissionGroups
                    .filter(
                      (group) =>
                        group.id !== "delete" || deletionProtection === false,
                    )
                    .every(
                      (group) =>
                        permissionGroupState(selectedPermissions, group) ===
                        true,
                    )
                    ? "取消全选"
                    : "全选"}
                </button>
                <div className="permissions">
                  {permissionGroups.map((group) => {
                    const state = permissionGroupState(
                      selectedPermissions,
                      group,
                    );
                    const protectedDelete =
                      group.id === "delete" && deletionProtection !== false;
                    return (
                      <div key={group.id} className="permission-option">
                        <label title={group.description}>
                          <Checkbox
                            aria-label={group.title}
                            checked={state}
                            disabled={!mcpEnabled || busy || protectedDelete}
                            onCheckedChange={(checked) =>
                              setServer((current) =>
                                togglePermissionGroup(
                                  current,
                                  group,
                                  checked === true,
                                ),
                              )
                            }
                          />
                          <span className="permission-option-text">
                            <span>{group.title}</span>
                            {state === "indeterminate" && (
                              <span className="field-help">部分授权</span>
                            )}
                            {protectedDelete && (
                              <span className="field-help">
                                {deletionProtection === null
                                  ? "删除保护状态未就绪"
                                  : (state ? "已授权，" : "") +
                                    "受全局删除保护限制"}
                              </span>
                            )}
                          </span>
                        </label>
                      </div>
                    );
                  })}
                </div>
                {deletionProtection && onOpenSettings && (
                  <button
                    type="button"
                    className="text-btn"
                    onClick={onOpenSettings}
                  >
                    前往删除保护设置
                  </button>
                )}
                {settingsError && (
                  <p role="alert" className="error-inline">
                    {settingsError}
                  </p>
                )}
              </div>
            </fieldset>
          </div>
          {error && <div className="error-inline">{error}</div>}
        </div>
        <div className="modal-foot">
          <Button
            variant="outline"
            size="sm"
            type="button"
            className="btn"
            onClick={onClose}
          >
            取消
          </Button>
          <Button
            variant="default"
            size="sm"
            className="btn primary"
            disabled={busy}
          >
            {busy ? "正在保存…" : "保存服务器"}
            <ArrowRight size={15} />
          </Button>
        </div>
      </form>
    </ModalFrame>
  );
}
