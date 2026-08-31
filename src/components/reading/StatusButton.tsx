import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  READING_STATUS_QUERY_KEY,
  fetchReadingStatuses,
  putReadingStatus,
} from "../../api/readingStatus";
import { cycleStatus, statusLabel, type ReadingStatus } from "../../lib/readingStatus";

export function StatusButton({ sourceId }: { sourceId: string }) {
  const queryClient = useQueryClient();
  const { data: statuses } = useQuery({
    queryKey: READING_STATUS_QUERY_KEY,
    queryFn: fetchReadingStatuses,
  });
  const status = statuses?.[sourceId];
  const cycle = useMutation({
    mutationFn: (next: ReadingStatus | undefined) =>
      putReadingStatus(sourceId, next ?? "unread"),
    // Optimistic: the pill must flip on click (parity with the old local
    // store); the settle-time invalidation reconciles with the backend.
    onMutate: (next) => {
      queryClient.setQueryData(
        READING_STATUS_QUERY_KEY,
        (cur: Record<string, ReadingStatus> | undefined) => {
          const map = { ...cur };
          if (next === undefined) delete map[sourceId];
          else map[sourceId] = next;
          return map;
        }
      );
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: READING_STATUS_QUERY_KEY }),
  });
  const color =
    status === "read"
      ? "var(--color-success)"
      : status === "reading"
        ? "var(--color-accent)"
        : "var(--color-muted)";
  return (
    <button
      type="button"
      onClick={(e) => { e.stopPropagation(); cycle.mutate(cycleStatus(status)); }}
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
