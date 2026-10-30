import {
  Children,
  isValidElement,
  type ReactNode,
  type ReactElement,
} from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
interface SelectFieldProps {
  value: string;
  onValueChange: (value: string) => void;
  children: ReactNode;
  disabled?: boolean;
  required?: boolean;
  name?: string;
  className?: string;
  "aria-label"?: string;
}
export function SelectField({
  value,
  onValueChange,
  children,
  disabled,
  required,
  name,
  className,
  "aria-label": label,
}: SelectFieldProps) {
  const options = Children.toArray(children).filter(
    isValidElement,
  ) as ReactElement<{
    value?: string;
    children: ReactNode;
    disabled?: boolean;
  }>[];
  const placeholder = options.find((option) => option.props.value === "")?.props
    .children;
  return (
    <Select
      value={value}
      onValueChange={onValueChange}
      disabled={disabled}
      required={required}
      name={name}
    >
      <SelectTrigger
        aria-label={label}
        className={"app-select " + (className || "")}
      >
        <SelectValue placeholder={placeholder || "请选择"}>
          {
            options.find((option) => String(option.props.value) === value)
              ?.props.children
          }
        </SelectValue>
      </SelectTrigger>
      <SelectContent>
        {options.map((option) => {
          const key = String(option.props.value ?? option.props.children);
          return key ? (
            <SelectItem key={key} value={key} disabled={option.props.disabled}>
              {option.props.children}
            </SelectItem>
          ) : null;
        })}
      </SelectContent>
    </Select>
  );
}
