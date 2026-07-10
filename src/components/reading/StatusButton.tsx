import { useReadingStatusStore } from "../../stores/readingStatus";
import { statusLabel } from "../../lib/readingStatus";

export function StatusButton({ sourceId }: { sourceId: string }) {
  const status = useReadingStatusStore((s) => s.statuses[sourceId]);
  const cycle = useReadingStatusStore((s) => s.cycle);
  const color =
    status === "read"
      ? "var(--color-success)"
      : status === "reading"
        ? "var(--color-accent)"
        : "var(--color-muted)";
  return (
    <button
      type="button"
      onClick={(e) => { e.stopPropagation(); cycle(sourceId); }}
      title="Cycle status: unread → reading → read → unread"
      className="font-mono font-medium shrink-0 self-start cursor-pointer"
      style={{
        fontSize: "10.5px",
        padding: "3px 10px",
        borderRadius: 20,
        border: `1px solid ${color}`,
        color,
        background: `color-mix(in srgb, ${color} 12%, transparent)`,
      }}
    >
      {statusLabel(status)}
    </button>
  );
}
