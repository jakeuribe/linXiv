import { useId, useState } from "react";
import type { ReactNode, Ref } from "react";

import { Button } from "../ui/button";
import { Input } from "../ui/input";
import type { GraphView } from "../../lib/graph/model";
import { normTag } from "../../lib/graph/model";
import type { GraphFilterState, TagRow } from "../../lib/graph/filter";
import {
  activeFilterSummary,
  activeTagFilterSummary,
  projectsMatchingName,
  projectsWithTag,
} from "../../lib/graph/filter";
import type { ForceSettings } from "../../lib/graph/layout";
import { DEFAULT_FORCES } from "../../lib/graph/layout";

/** The right-hand panel column: Layout, Filters, Tag Filter and Selection —
 *  plain React state, so the panels are a function of the filter. */

/** How many colour dots one filter row shows before it collapses the rest into
 *  a "+n". Three keeps the label readable in the panel column's width. */
const MAX_ROW_SWATCHES = 3;

export interface GraphPanelsProps {
  /** The column's own element, so the page can measure what it covers and let
   *  the canvas frame into the strip that is left. */
  columnRef?: Ref<HTMLDivElement>;
  view: GraphView;
  filter: GraphFilterState;
  onFilterChange: (next: GraphFilterState) => void;
  onClearFilters: () => void;
  forces: ForceSettings;
  onForcesChange: (next: ForceSettings) => void;
  onRelayout: () => void;
  selectedCount: number;
  /** Selected papers the current filter state does not DRAW at all. Painting the
   *  highlight covers an ordinary 8% ghost, but these are genuinely off the
   *  canvas, so the count itself is the only honest place left to report them. */
  hiddenSelectedCount: number;
  onSelectAllVisible: () => void;
  onClearSelection: () => void;
}

export default function GraphPanels({
  columnRef,
  view,
  filter,
  onFilterChange,
  onClearFilters,
  forces,
  onForcesChange,
  onRelayout,
  selectedCount,
  hiddenSelectedCount,
  onSelectAllVisible,
  onClearSelection,
}: GraphPanelsProps) {
  const patch = (next: Partial<GraphFilterState>) => onFilterChange({ ...filter, ...next });

  return (
    <div
      ref={columnRef}
      className="absolute top-0 right-0 z-10 flex w-[260px] max-h-full flex-col gap-2 overflow-y-auto p-3"
    >
      <Panel title="Layout" defaultOpen>
        <SectionTitle>Forces</SectionTitle>
        <Slider
          label="Center force"
          min={0}
          max={1}
          step={0.01}
          value={forces.center}
          onChange={(center) => onForcesChange({ ...forces, center })}
        />
        <Slider
          label="Repel force"
          min={0}
          max={1000}
          step={10}
          value={forces.repel}
          onChange={(repel) => onForcesChange({ ...forces, repel })}
        />
        <Slider
          label="Link distance"
          min={10}
          max={300}
          step={5}
          value={forces.linkDistance}
          onChange={(linkDistance) => onForcesChange({ ...forces, linkDistance })}
        />
        <Slider
          label="Link strength"
          min={0}
          max={1}
          step={0.01}
          value={forces.linkStrength}
          onChange={(linkStrength) => onForcesChange({ ...forces, linkStrength })}
        />
        <Button variant="muted" size="sm" className="mt-2 w-full" onClick={onRelayout}>
          Randomize &amp; restart
        </Button>
        {!sameForces(forces, DEFAULT_FORCES) && (
          <Button
            variant="ghost"
            size="sm"
            className="mt-1 w-full"
            onClick={() => onForcesChange(DEFAULT_FORCES)}
          >
            Reset forces
          </Button>
        )}
      </Panel>

      <Panel title="Filters" badge={activeFilterSummary(filter)}>
        <SectionTitle>Visibility</SectionTitle>
        <div className="flex flex-wrap gap-x-3 gap-y-1">
          <Check
            label="Papers"
            checked={filter.showPapers}
            onChange={(showPapers) => patch({ showPapers })}
          />
          <Check
            label="Authors"
            checked={filter.showAuthors}
            onChange={(showAuthors) => patch({ showAuthors })}
          />
          <Check
            label="Tags"
            checked={filter.showTags}
            onChange={(showTags) => patch({ showTags })}
          />
        </div>

        <SectionTitle>Attributes</SectionTitle>
        <Field label="Category">
          <DatalistInput
            value={filter.category}
            placeholder="type to filter…"
            options={view.categories}
            onChange={(category) => patch({ category })}
          />
        </Field>
        <Check
          label="Has PDF only"
          checked={filter.hasPdf}
          onChange={(hasPdf) => patch({ hasPdf })}
        />

        {/* type=date, as the rest of the app enters dates. These were free text,
            so typing "2024-01-05" walked the graph through "2", "20", "202"…
            each a live filter over every paper, and a typo like "05/01/2024"
            filtered silently and wrongly with nothing to say so. */}
        <SectionTitle>Date range</SectionTitle>
        <Field label="From">
          <Input
            type="date"
            className="py-1 text-xs"
            value={filter.dateFrom}
            onChange={(e) => patch({ dateFrom: e.target.value })}
          />
        </Field>
        <Field label="To">
          <Input
            type="date"
            className="py-1 text-xs"
            value={filter.dateTo}
            onChange={(e) => patch({ dateTo: e.target.value })}
          />
        </Field>

        <SectionTitle>Highlight</SectionTitle>
        <Field label="Title">
          <Input
            className="py-1 text-xs"
            placeholder="e.g. transformer"
            value={filter.title}
            onChange={(e) => patch({ title: e.target.value })}
          />
        </Field>
        <Field label="Author">
          <Input
            className="py-1 text-xs"
            placeholder="e.g. Hinton"
            value={filter.author}
            onChange={(e) => patch({ author: e.target.value })}
          />
        </Field>

        <Button
          variant={filter.isolate ? "primary" : "muted"}
          size="sm"
          className="mt-2 w-full"
          aria-pressed={filter.isolate}
          onClick={() => patch({ isolate: !filter.isolate })}
        >
          Show highlighted only
        </Button>
        <Button variant="ghost" size="sm" className="mt-1 w-full" onClick={onClearFilters}>
          Clear all filters
        </Button>
      </Panel>

      <Panel title="Tag Filter" badge={activeTagFilterSummary(filter)}>
        <SectionTitle>Projects</SectionTitle>
        <RowList
          placeholder="type a project…"
          addTitle="Add project filter"
          // Only projects with a paper on THIS canvas are offered: both boxes
          // match a paper through its own project ids, so an active-but-absent
          // project is a suggestion whose only possible effect is to empty the
          // canvas. A name typed by hand still resolves and still filters — it
          // is just marked when it stands for nothing.
          options={view.projects.filter((p) => p.on_graph).map((p) => p.name)}
          rows={filter.projectNames}
          onChange={(projectNames) => patch({ projectNames })}
          emptyText="No project filters active."
          resolve={(name) => projectsMatchingName(view, name)}
          noPapersTitle="No paper on this graph belongs to that project."
        />

        <SectionTitle>Project Tags</SectionTitle>
        <RowList
          placeholder="type a tag…"
          addTitle="Add project tag filter"
          options={projectTagOptions(view)}
          rows={filter.projectTags}
          onChange={(projectTags) => patch({ projectTags })}
          emptyText="No project tag filters active."
          resolve={(tag) => projectsWithTag(view, tag)}
          noPapersTitle="No paper on this graph belongs to a project with this tag."
        />

        <SectionTitle>Paper Tags</SectionTitle>
        <TagRowList
          view={view}
          rows={filter.tagRows}
          onChange={(tagRows) => patch({ tagRows })}
        />
      </Panel>

      <Panel
        title={
          hiddenSelectedCount > 0
            ? `Selection (${selectedCount}, ${hiddenSelectedCount} hidden)`
            : `Selection (${selectedCount})`
        }
      >
        <p className="mb-2 text-[11px] text-muted">Click to select · Ctrl+click to add</p>
        <Button variant="muted" size="sm" className="w-full" onClick={onSelectAllVisible}>
          Select all visible
        </Button>
        <Button variant="ghost" size="sm" className="mt-1 w-full" onClick={onClearSelection}>
          Clear selection
        </Button>
      </Panel>
    </div>
  );
}

/** Every project tag currently offerable: from the projects that have a paper on
 *  this canvas. The reserved reading-list marker is already filtered out by the
 *  backend, so nothing here has to know about it. */
function projectTagOptions(view: GraphView): string[] {
  const out = new Set<string>();
  for (const p of view.projects) {
    if (!p.on_graph) continue;
    for (const t of p.tags) out.add(t);
  }
  return [...out].sort();
}

function sameForces(a: ForceSettings, b: ForceSettings): boolean {
  return (
    a.center === b.center &&
    a.repel === b.repel &&
    a.linkDistance === b.linkDistance &&
    a.linkStrength === b.linkStrength
  );
}

/**
 * A collapsible panel. `badge` is the list of things this panel currently has
 * switched on: both filter panels open COLLAPSED and their state outlives every
 * navigation, so an active filter used to be a canvas of 8% ghosts with the
 * control that caused it two clicks away behind a "▶" and nothing on the header
 * to say so.
 */
function Panel({
  title,
  badge,
  defaultOpen = false,
  children,
}: {
  title: string;
  badge?: string[];
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  const count = badge?.length ?? 0;
  return (
    <div
      className="rounded-md border border-border shadow-sm"
      style={{ backgroundColor: "var(--color-panel)" }}
    >
      <button
        type="button"
        className="flex w-full items-center justify-between px-3 py-2 text-xs font-semibold text-text"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen((o) => !o)}
      >
        <span>
          {title}
          {count > 0 && (
            <span className="text-muted font-normal" title={badge!.join("\n")}>
              {` (${count})`}
            </span>
          )}
        </span>
        <span aria-hidden>{open ? "▼" : "▶"}</span>
      </button>
      {open && (
        <div id={bodyId} className="border-t border-border px-3 py-2">
          {children}
        </div>
      )}
    </div>
  );
}

function SectionTitle({ children }: { children: ReactNode }) {
  return (
    <div className="mt-2 mb-1 text-[10px] font-semibold uppercase tracking-wide text-muted first:mt-0">
      {children}
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="mb-1 flex items-center gap-2 text-xs text-muted">
      <span className="w-[68px] shrink-0">{label}</span>
      {children}
    </label>
  );
}

function Check({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer select-none items-center gap-1.5 text-xs text-muted">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      {label}
    </label>
  );
}

function Slider({
  label,
  min,
  max,
  step,
  value,
  onChange,
}: {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  onChange: (next: number) => void;
}) {
  return (
    <label className="mb-1 flex items-center gap-2 text-xs text-muted">
      <span className="w-[78px] shrink-0">{label}</span>
      <input
        type="range"
        className="min-w-0 flex-1"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(parseFloat(e.target.value))}
      />
      <span className="w-[34px] shrink-0 text-right tabular-nums text-text">{value}</span>
    </label>
  );
}

function DatalistInput({
  value,
  placeholder,
  options,
  onChange,
}: {
  value: string;
  placeholder?: string;
  options: readonly string[];
  onChange: (next: string) => void;
}) {
  const listId = useId();
  return (
    <>
      <Input
        className="py-1 text-xs"
        list={listId}
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
      <datalist id={listId}>
        {options.map((o) => (
          <option key={o} value={o} />
        ))}
      </datalist>
    </>
  );
}

/** The add-box shared by all three row lists. */
function AddRow({
  placeholder,
  addTitle,
  options,
  onAdd,
}: {
  placeholder: string;
  addTitle: string;
  options: readonly string[];
  onAdd: (value: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const submit = () => {
    const value = draft.trim();
    if (!value) {
      setDraft("");
      return;
    }
    onAdd(value);
    setDraft("");
  };
  const listId = useId();
  return (
    <div className="mb-1 flex items-center gap-1">
      <Input
        className="py-1 text-xs"
        list={listId}
        autoComplete="off"
        value={draft}
        placeholder={placeholder}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          e.preventDefault();
          submit();
        }}
      />
      <datalist id={listId}>
        {options.map((o) => (
          <option key={o} value={o} />
        ))}
      </datalist>
      <Button variant="muted" size="sm" title={addTitle} onClick={submit}>
        +
      </Button>
    </div>
  );
}

/**
 * The Projects / Project Tags lists. Each row is free text, so it can stand for
 * nothing in two ways the canvas cannot tell apart — it resolves to no project
 * at all (a typo, or a project renamed or deleted since the row was added), or
 * it resolves to projects none of whose papers are on this graph. Both are
 * marked; the swatches are what tell them apart.
 */
function RowList({
  placeholder,
  addTitle,
  options,
  rows,
  onChange,
  emptyText,
  resolve,
  noPapersTitle,
}: {
  placeholder: string;
  addTitle: string;
  options: readonly string[];
  rows: string[];
  onChange: (next: string[]) => void;
  emptyText: string;
  resolve: (value: string) => { id: number; name: string; color: string; on_graph: boolean }[];
  noPapersTitle: string;
}) {
  const add = (value: string) => {
    // Both lists match case-insensitively downstream, so "ML" after "ml" would
    // add a second row that filters identically.
    if (rows.some((r) => r.toLowerCase() === value.toLowerCase())) return;
    onChange([...rows, value]);
  };
  return (
    <>
      <AddRow placeholder={placeholder} addTitle={addTitle} options={options} onAdd={add} />
      {rows.length === 0 ? (
        <p className="text-[11px] italic text-muted">{emptyText}</p>
      ) : (
        rows.map((name, i) => {
          const matched = resolve(name);
          const drawing = matched.filter((p) => p.on_graph);
          return (
            <FilterRow
              key={`${name}-${i}`}
              label={name}
              unmatched={drawing.length === 0}
              title={matched.length > 0 && drawing.length === 0 ? noPapersTitle : undefined}
              lead={<Swatches projects={matched} />}
              onRemove={() => onChange(rows.filter((_, j) => j !== i))}
            />
          );
        })
      )}
    </>
  );
}

function Swatches({ projects }: { projects: { name: string; color: string }[] }) {
  if (projects.length === 0) {
    return (
      <span
        className="w-[34px] shrink-0 text-center text-xs"
        style={{ color: "var(--color-danger)" }}
        title="Matches no project"
      >
        ✗
      </span>
    );
  }
  return (
    <span
      className="flex w-[34px] shrink-0 items-center gap-0.5"
      title={projects.map((p) => p.name).join(", ")}
    >
      {projects.slice(0, MAX_ROW_SWATCHES).map((p, i) => (
        <span
          key={`${p.name}-${i}`}
          className="inline-block h-2 w-2 rounded-full"
          style={{ backgroundColor: p.color }}
        />
      ))}
      {projects.length > MAX_ROW_SWATCHES && (
        <span className="text-[9px] text-muted">+{projects.length - MAX_ROW_SWATCHES}</span>
      )}
    </span>
  );
}

/**
 * The Paper Tags logic builder: rows joined by a per-row AND/OR toggle.
 *
 * A row is marked when no paper on this graph carries the tag — free text, so a
 * typo (or a tag renamed, merged or deleted elsewhere since) matches nothing and
 * filters the canvas to nothing while reading exactly like a working row.
 */
function TagRowList({
  view,
  rows,
  onChange,
}: {
  view: GraphView;
  rows: TagRow[];
  onChange: (next: TagRow[]) => void;
}) {
  // Exactly the universe a row is compared against: the tags carried by a paper
  // on THIS canvas. Offering from a wider list (the whole TAG table, which keeps
  // rows a paper dropped its last link to) meant a dropdown entry whose only
  // possible effect was to empty the graph and mark itself unmatched.
  const keys = new Set(view.tags.map((t) => t.key));
  const add = (value: string) => {
    if (rows.some((r) => normTag(r.tag) === normTag(value))) return;
    onChange([...rows, { op: "AND", tag: value }]);
  };
  return (
    <>
      <AddRow
        placeholder="type a tag…"
        addTitle="Add tag"
        options={view.tags.map((t) => t.label)}
        onAdd={add}
      />
      {rows.length === 0 ? (
        <p className="text-[11px] italic text-muted">No tag filters active.</p>
      ) : (
        rows.map((row, i) => (
          <FilterRow
            key={`${row.tag}-${i}`}
            label={row.tag}
            unmatched={!keys.has(normTag(row.tag))}
            title={
              keys.has(normTag(row.tag)) ? undefined : "No paper on this graph carries this tag."
            }
            lead={
              i === 0 ? (
                <span className="w-[34px] shrink-0" />
              ) : (
                <button
                  type="button"
                  className="w-[34px] shrink-0 rounded border border-border text-[10px] font-semibold text-text"
                  title="Click to toggle AND / OR"
                  onClick={() =>
                    onChange(
                      rows.map((r, j) =>
                        j === i ? { ...r, op: r.op === "AND" ? "OR" : "AND" } : r
                      )
                    )
                  }
                >
                  {row.op}
                </button>
              )
            }
            onRemove={() => onChange(rows.filter((_, j) => j !== i))}
          />
        ))
      )}
    </>
  );
}

function FilterRow({
  label,
  unmatched,
  title,
  lead,
  onRemove,
}: {
  label: string;
  unmatched: boolean;
  title?: string;
  lead: ReactNode;
  onRemove: () => void;
}) {
  return (
    <div
      className="mb-1 flex items-center gap-1.5 text-xs"
      title={title}
      style={unmatched ? { opacity: 0.55, textDecoration: "line-through" } : undefined}
    >
      {lead}
      <span className="min-w-0 flex-1 truncate text-text">{label}</span>
      <button
        type="button"
        className="shrink-0 px-1 text-muted hover:text-[var(--color-danger)]"
        title="Remove"
        aria-label={`Remove ${label}`}
        onClick={onRemove}
      >
        ×
      </button>
    </div>
  );
}
