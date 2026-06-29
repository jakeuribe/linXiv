export type SortDir = "asc" | "desc";

interface SortHeaderProps {
  label: string;
  /** True when the table is currently sorted by this column. */
  active: boolean;
  dir: SortDir;
  onSort: () => void;
  align?: "left" | "right";
}

/** A clickable `<th>` that toggles sort on its column and shows the direction. */
export function SortHeader({ label, active, dir, onSort, align = "left" }: SortHeaderProps) {
  return (
    <th
      scope="col"
      aria-sort={active ? (dir === "asc" ? "ascending" : "descending") : "none"}
      className="py-2 font-medium select-none"
      style={{ textAlign: align, color: "var(--color-muted)" }}
    >
      <button
        type="button"
        onClick={onSort}
        className="inline-flex items-center gap-1 hover:text-[var(--color-text)] transition-colors"
        style={{ flexDirection: align === "right" ? "row-reverse" : "row" }}
      >
        {label}
        <span style={{ opacity: active ? 1 : 0.25, fontSize: "0.7em" }}>
          {active ? (dir === "asc" ? "▲" : "▼") : "▲"}
        </span>
      </button>
    </th>
  );
}

/** Cycle helper: clicking the active column flips direction; a new column starts at `initial`. */
export function nextSort<K>(
  current: { key: K; dir: SortDir },
  key: K,
  initial: SortDir = "asc",
): { key: K; dir: SortDir } {
  if (current.key === key) {
    return { key, dir: current.dir === "asc" ? "desc" : "asc" };
  }
  return { key, dir: initial };
}
