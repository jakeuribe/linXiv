interface SelectionBarProps {
  count: number;
  onAddToProject: () => void;
  onDelete: () => void;
  onClear: () => void;
}

export function SelectionBar({
  count,
  onAddToProject,
  onDelete,
  onClear,
}: SelectionBarProps) {
  if (count === 0) return null;

  return (
    <div
      className="lx-rise-pill fixed bottom-6 left-1/2 z-30 flex -translate-x-1/2 items-center gap-3 rounded-[40px] px-5 py-2.5"
      style={{
        backgroundColor: "#16161f",
        border: "1px solid rgba(255,255,255,0.08)",
        boxShadow: "0 24px 70px rgba(0,0,0,0.45)",
        color: "#ffffff",
      }}
    >
      <span className="whitespace-nowrap text-sm font-medium">
        {count} selected
      </span>
      <span aria-hidden className="h-4 w-px" style={{ backgroundColor: "rgba(255,255,255,0.15)" }} />
      <div className="flex items-center gap-1">
        <button
          onClick={onAddToProject}
          className="rounded px-2.5 py-1 text-xs font-medium transition-colors hover:bg-white/10"
          style={{ color: "#ffffff" }}
        >
          Add to Project
        </button>
        <button
          onClick={onDelete}
          className="rounded px-2.5 py-1 text-xs font-medium transition-colors hover:bg-white/10"
          style={{ color: "var(--color-danger)" }}
        >
          Delete
        </button>
        <button
          onClick={onClear}
          className="rounded px-2.5 py-1 text-xs font-medium transition-colors hover:bg-white/10"
          style={{ color: "rgba(255,255,255,0.65)" }}
        >
          Done
        </button>
      </div>
    </div>
  );
}
