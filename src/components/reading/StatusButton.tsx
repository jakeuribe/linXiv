import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  READING_STATUS_QUERY_KEY,
  fetchReadingStatuses,
  putReadingStatus,
} from "../../api/readingStatus";
import { cycleStatus, statusLabel, type ReadingStatus } from "../../lib/readingStatus";

/** Set a paper's reading status (undefined = unread). Optimistic: the pill
 *  must flip on click (parity with the old local store); the settle-time
 *  invalidation reconciles with the backend. Shared by the pill button and the
 *  reading-queue context menu. */
export function useSetReadingStatus() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ sourceId, status }: { sourceId: string; status: ReadingStatus | undefined }) =>
      putReadingStatus(sourceId, status ?? "unread"),
    onMutate: ({ sourceId, status }) => {
      queryClient.setQueryData(
        READING_STATUS_QUERY_KEY,
        (cur: Record<string, ReadingStatus> | undefined) => {
          const map = { ...cur };
          if (status === undefined) delete map[sourceId];
          else map[sourceId] = status;
          return map;
        }
      );
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: READING_STATUS_QUERY_KEY }),
  });
}

export function StatusButton({ sourceId }: { sourceId: string }) {
  const { data: statuses } = useQuery({
    queryKey: READING_STATUS_QUERY_KEY,
    queryFn: fetchReadingStatuses,
  });
  const status = statuses?.[sourceId];
  const cycle = useSetReadingStatus();
  const color =
    status === "read"
      ? "var(--color-success)"
      : status === "reading"
        ? "var(--color-accent)"
        : "var(--color-muted)";
  return (
    <button
      type="button"
      onClick={(e) => { e.stopPropagation(); cycle.mutate({ sourceId, status: cycleStatus(status) }); }}
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
