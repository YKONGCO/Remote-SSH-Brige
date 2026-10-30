export function CompactTime({ value }: { value: string | number }) {
  const date = new Date(value);
  return (
    <time
      className="compact-time"
      dateTime={date.toISOString()}
      title={date.toLocaleString("zh-CN", { hour12: false })}
    >
      <span className="compact-date">{date.toLocaleDateString("zh-CN")}</span>
      <span className="compact-clock">
        {date.toLocaleTimeString("zh-CN", { hour12: false })}
      </span>
    </time>
  );
}
