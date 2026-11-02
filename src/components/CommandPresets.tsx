import { useState } from "react";
import {
  Search,
  Plus,
  Check,
  Layers,
  TerminalSquare,
  FileSearch,
  Code2,
  Braces,
  Cpu,
  FlaskConical,
  GitBranch,
  SearchX,
  type LucideIcon,
} from "lucide-react";
import {
  commandPresets,
  defaultAllowedCommands,
  parseCommandList,
  validCommandLists,
  MAX_COMMAND_RULES,
} from "../runtime-settings";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

const categoryIcons: Record<string, LucideIcon> = {
  system: TerminalSquare,
  data: FileSearch,
  python: Code2,
  javascript: Braces,
  ml: Cpu,
  science: FlaskConical,
  build: GitBranch,
};

export function CommandPresets({
  value,
  disabled,
  onChange,
  onError,
}: {
  value: string;
  disabled: boolean;
  onChange: (value: string) => void;
  onError: (message: string) => void;
}) {
  const [search, setSearch] = useState("");
  const [activeCategory, setActiveCategory] = useState("python");
  const selected = new Set(parseCommandList(value));
  const query = search.trim().toLowerCase();
  const groups = commandPresets
    .map((preset) => ({
      ...preset,
      commands: preset.label.toLowerCase().includes(query)
        ? preset.commands
        : preset.commands.filter((command) =>
            command.toLowerCase().includes(query),
          ),
    }))
    .filter((preset) => preset.commands.length);
  const active =
    groups.find((preset) => preset.id === activeCategory) ?? groups[0];
  const missing =
    active?.commands.filter((name) => !selected.has(name)).length ?? 0;
  const totalSelected = defaultAllowedCommands.filter((name) =>
    selected.has(name),
  ).length;
  const allSelected = totalSelected === defaultAllowedCommands.length;
  const toggle = (commands: string[], select: boolean) => {
    if (disabled) return;
    const removed = new Set(commands);
    const merged = select
      ? [...new Set([...selected, ...commands])]
      : [...selected].filter((command) => !removed.has(command));
    if (select && !validCommandLists(merged, [])) {
      onError(
        `白名单最多 ${MAX_COMMAND_RULES} 项；请先修正名单中的参数、通配符或 Shell 语法。`,
      );
      return;
    }
    onChange(merged.join("\n"));
    onError("");
  };

  return (
    <section className="command-presets" aria-label="常用命令预设">
      <div className="command-presets-toolbar">
        <div className="command-preset-search">
          <Search size={16} aria-hidden="true" />
          <Input
            aria-label="搜索常用命令"
            placeholder="搜索命令或分类…"
            value={search}
            disabled={disabled}
            onChange={(event) => setSearch(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.preventDefault();
            }}
          />
          <span>{defaultAllowedCommands.length} 个预设</span>
        </div>
        <Button
          type="button"
          variant="outline"
          className="command-fill-presets"
          disabled={disabled}
          aria-pressed={allSelected}
          title={
            allSelected
              ? "取消全部预设命令，保留自定义命令"
              : "选中全部预设命令"
          }
          onClick={() => toggle(defaultAllowedCommands, !allSelected)}
        >
          <Layers size={14} />
          {allSelected ? "取消全选" : "全选常用命令"}
        </Button>
      </div>
      <div className="command-preset-workspace">
        <nav className="command-preset-nav" aria-label="命令分类">
          {groups.map((preset) => {
            const Icon = categoryIcons[preset.id] ?? Code2;
            return (
              <button
                type="button"
                key={preset.id}
                className="command-category"
                title={preset.label}
                aria-pressed={active?.id === preset.id}
                disabled={disabled}
                onClick={() => setActiveCategory(preset.id)}
              >
                <Icon size={16} aria-hidden="true" />
                <span>{preset.label}</span>
                <small>{preset.commands.length}</small>
              </button>
            );
          })}
        </nav>
        <div className="command-preset-panel">
          {active ? (
            <>
              <div className="command-preset-panel-head">
                <div>
                  <h3>{active.label}</h3>
                  <span className="command-group-count">
                    {active.commands.length}
                  </span>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={disabled}
                  aria-pressed={!missing}
                  aria-label={`${missing ? "全选" : "取消全选"}${active.label}${query ? "搜索结果" : ""}`}
                  onClick={() => toggle(active.commands, !!missing)}
                >
                  {missing ? <Plus size={14} /> : <Check size={14} />}
                  {missing ? "全选本组" : "取消本组选中"}
                </Button>
              </div>
              <div className="command-preset-commands">
                {active.commands.map((command) => (
                  <button
                    key={command}
                    type="button"
                    className={`command-chip${selected.has(command) ? " is-selected" : ""}`}
                    aria-label={`${selected.has(command) ? "取消命令" : "添加命令"} ${command}`}
                    aria-pressed={selected.has(command)}
                    title={
                      selected.has(command)
                        ? `取消 ${command}`
                        : `添加 ${command}`
                    }
                    disabled={disabled}
                    onClick={() => toggle([command], !selected.has(command))}
                  >
                    <code>{command}</code>
                    {selected.has(command) ? (
                      <Check size={13} />
                    ) : (
                      <Plus size={13} />
                    )}
                  </button>
                ))}
              </div>
            </>
          ) : (
            <div className="command-preset-empty">
              <SearchX size={28} />
              <strong>未找到匹配的命令</strong>
              <p>换个关键词，或在下方添加自定义命令。</p>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
