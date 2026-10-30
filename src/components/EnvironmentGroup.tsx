import { useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { Input } from "./ui/input";
import { Button } from "./ui/button";

export function EnvironmentGroup({
  value,
  groups,
  onChange,
}: {
  value: string;
  groups: string[];
  onChange: (value: string) => void;
}) {
  const field = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [width, setWidth] = useState<number>();
  const options = Array.from(
    new Set(
      [
        "生产环境",
        "开发环境",
        "测试环境",
        "预发布环境",
        "集成环境",
        "演示环境",
        "沙箱环境",
        "本地环境",
        "灾备环境",
        "运维环境",
        ...groups,
        value,
      ].filter(Boolean),
    ),
  );
  const showMenu = (next: boolean) => {
    if (next)
      setWidth(field.current?.getBoundingClientRect().width || undefined);
    setOpen(next);
  };
  return (
    <div className="environment-group-field" ref={field}>
      <Input
        ref={input}
        required
        aria-label="环境分组"
        placeholder="选择或输入分组"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            showMenu(true);
          }
        }}
      />
      <DropdownMenu open={open} onOpenChange={showMenu}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="environment-group-trigger"
            aria-label="选择环境分组"
          >
            <ChevronDown size={13} />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          className="environment-group-menu"
          style={{ width }}
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            input.current?.focus();
          }}
        >
          {options.map((group) => (
            <DropdownMenuItem
              key={group}
              aria-current={group === value ? "true" : undefined}
              onSelect={() => onChange(group)}
            >
              <span>{group}</span>
              {group === value && <Check size={14} aria-hidden="true" />}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
