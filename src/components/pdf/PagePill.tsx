// Bottom-center page stepper over the rendered react-pdf pages.
export function PagePill({
  page,
  total,
  onGo,
}: {
  page: number;
  total: number;
  onGo: (n: number) => void;
}) {
  if (total <= 0) return null;
  return (
    <div className="absolute bottom-4 left-1/2 -translate-x-1/2 z-10 flex items-center gap-2 rounded-full bg-panel border border-border shadow-card px-3 py-1.5">
      <button
        className="text-muted hover:text-text disabled:opacity-40 disabled:pointer-events-none px-1"
        onClick={() => onGo(page - 1)}
        disabled={page <= 1}
        aria-label="Previous page"
      >
        ‹
      </button>
      <span className="font-mono text-xs text-text tabular-nums">
        {page} / {total}
      </span>
      <button
        className="text-muted hover:text-text disabled:opacity-40 disabled:pointer-events-none px-1"
        onClick={() => onGo(page + 1)}
        disabled={page >= total}
        aria-label="Next page"
      >
        ›
      </button>
    </div>
  );
}
