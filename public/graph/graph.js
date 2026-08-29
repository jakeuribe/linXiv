// Node-type colours. Papers, tags and the highlight track the host palette
// (accent / success / danger) and are refreshed by _syncNodeColors() at
// bootstrap and on every theme_update -- they used to be hardcoded copies of
// the Navy-dark preset, so under any other theme the graph painted colours the
// rest of the app had stopped using. The single set of literal fallbacks lives
// in getThemeColors(), for a standalone load with no --color-* vars set.
// Authors get no colour of their own because theme.ts has no fourth semantic
// token; node type is also encoded by shape (paper = ellipse, author =
// diamond, tag = roundrectangle), so this fixed hue only has to read as "not
// the accent".
let PAPER_COLOR;
let TAG_COLOR;
let HIGHLIGHT_COLOR;
const AUTHOR_COLOR    = '#e8a838';

// The tokens src/lib/theme.ts's getColors() returns, and the ones graph.html's
// pre-paint script sets off the src. Keep the two lists in step.
const THEME_VARS = ['bg', 'panel', 'border', 'accent', 'text', 'muted', 'success', 'danger'];
// The two `color-scheme` values src/styles/tokens.css sets from `[data-mode]`.
// The iframe is a separate document, so it inherits none of that: without an
// explicit value it stays on the UA default (`light`) and WebKitGTK draws every
// native control -- the date pickers, the checkboxes, the force sliders, the
// #right-panels scrollbar -- in the OS light theme on top of a dark panel.
// tokens.css:13 documents that exact failure for the host document.
const COLOR_SCHEMES = ['dark', 'light'];
// The app's typeface, kept identical to the `html, body, #root` stack in
// src/styles/globals.css (graph.css re-declares the same self-hosted @font-face
// rules, since the iframe is a separate document). Cytoscape wants a bare CSS
// font-family string, so no quoting. Pinned equal to the host's in
// graphIframeAssets.test.ts.
const LABEL_FONT_FAMILY = 'Inter';
const LABEL_FONT        = LABEL_FONT_FAMILY + ', system-ui, sans-serif';
// Longest a stalled webfont request may hold up the first render.
const FONT_LOAD_TIMEOUT_MS = 3000;

const DIM_OPACITY     = 0.08;   // filter dim (isolate / non-matching)
const SEL_DIM_OPACITY = 0.28;   // softer dim for non-selected nodes
const FULL_OPACITY    = 1.0;

// ── Reproducible layout (opt-in) ─────────────────────────────────────────────
// Initial node positions are normally Math.random() -- fine for real use, but
// it makes scripted demo recordings a lottery (framing/coordinates differ
// every run). If localStorage key LAYOUT_SEED_KEY holds a value, seed a small
// deterministic PRNG (mulberry32) from it instead; absent (the default for
// every real user), nothing changes. Re-seeded fresh on each loadGraph()/
// relayout so "same seed" means "same sequence" regardless of reload count.
const LAYOUT_SEED_KEY = 'linxiv-graph-seed';

function mulberry32(seed) {
    let a = seed >>> 0;
    return function() {
        a = (a + 0x6D2B79F5) | 0;
        let t = Math.imul(a ^ (a >>> 15), 1 | a);
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
}

function _layoutRng() {
    const seed = localStorage.getItem(LAYOUT_SEED_KEY);
    if (!seed) return Math.random;
    return mulberry32(parseInt(seed, 10) || 0);
}

let cy           = null;
let simulation   = null;
let _simNodeById  = new Map();
let _allEdgeDefs  = [];   // [{source: id, target: id}] — original, never mutated by D3
let _debounce    = null;
let _selectedIds  = new Set();
// paper node id → lowercased author labels, built in loadGraph. Author names
// only reach the client as separate author nodes joined by edges, so the
// "Author" highlight filter needs this index to test a paper in one step.
let _paperAuthorLabels = new Map();

// Filter state (needed so selection style can layer on top)
let _visiblePaperIds  = null;   // null = no filter active
let _visibleAuthorIds = null;
let _visibleTagIds    = null;
let _filterIsolate    = false;
// Node types the Visibility checkboxes turned off. Kept apart from the id sets
// above because "don't draw authors" is a different question from "which
// authors did the attribute filters match".
let _hiddenTypes      = new Set();
// The node ids the force layout is currently running over -- the union of
// filterGraph's matched sets, or null before the first filter pass ("every node
// participates"). Module-level because the charge and collision forces are
// built in five places between them and every one of those has to agree on it:
// an excluded node is PINNED, not removed from the simulation, so rebuilding
// its charge at full strength -- or its collision radius at full size -- lets
// it go on shoving the matching nodes around from behind its 8% ghost.
//
// The drag handlers read it for the other half of that rule: a ghost is still
// grabbable, so releasing one has to restore the filter's pin rather than the
// plain "back to the layout" a matching node gets.
let _layoutNodeIds    = null;

// Tag logic builder state: [{op: 'AND'|'OR', tag: string}]
let _tagRows = [];
// Every tag any paper in the CURRENT payload carries, run through _normTag —
// i.e. exactly the universe _evalTagFilter compares a row against, so a row
// this set does not hold is a row that matches no paper. Rebuilt by loadGraph;
// null before the first load, where nothing can be claimed either way.
let _paperTagSet = null;

// The labels `/api/tags` last answered with, indexed `_normTag(label)` -> the
// canonical spelling (trimmed), in the order the endpoint sent them. Held on
// the module for two reasons: so the Paper Tags datalist can be rebuilt when the
// PAPERS change and not only when that endpoint answers (see
// _renderTagDatalist), and so the tag CHIPS on the canvas can be drawn with the
// TAG table's spelling rather than the payload's (see _canonicalTagLabel). Null
// before the first successful reply — "no list has arrived yet", never "this
// library has no tags".
let _tagLabelByNorm = null;

// Every ACTIVE project id a paper in the CURRENT payload belongs to — i.e.
// exactly the project universe `filterGraph` can match, since it tests a
// paper's own `project_ids` array. Rebuilt by loadGraph; null before the first
// load, where nothing can be claimed either way. `/api/graph/project-options`
// answers with every active project whether or not any paper on this canvas is
// in one, so this is the narrower set — the Projects / Project Tags equivalent
// of _paperTagSet.
let _paperProjectIds = null;

let _projectMap = new Map();  // id → {name, color, tags[]}
// A fit that _fitGraph() had to skip because the viewport was 0x0 (hidden
// iframe); replayed by the resize handler once the container has real bounds.
let _fitDeferred = false;
// One-shot: reframe the next time the force simulation settles. Armed by a
// fresh load and by "Randomize & restart" -- both hand d3 a square of random
// seed positions that the layout then spreads well past, so the viewport in
// force at that moment frames something that no longer exists. Module-level
// rather than a loadGraph() local because the relayout button lives outside
// loadGraph and has to be able to re-arm it. Cleared on the first grab so a
// reframe never yanks the viewport out from under a drag.
let _fitOnSettle = false;

// Data-fetch state for in-place refresh / option changes (see fetchAndLoadGraph).
let _graphBase           = null;   // resolved API origin, or null when offline (file:)
let _excludeSingleAuthors = false; // current "hide single-paper authors" option
let _loadToken           = 0;      // guards against out-of-order fetch results

const $ = id => document.getElementById(id);

// ── Panel collapse wiring ────────────────────────────────────────────────────

document.querySelectorAll('.panel-toggle').forEach(btn => {
    const body = document.getElementById(btn.dataset.target);
    if (!body) return;
    btn.setAttribute('aria-controls', btn.dataset.target);
    btn.setAttribute('aria-expanded', String(body.style.display !== 'none'));
    btn.addEventListener('click', () => {
        const collapsed = body.style.display === 'none';
        body.style.display = collapsed ? '' : 'none';
        btn.textContent = collapsed ? '▼' : '▶';
        btn.setAttribute('aria-expanded', String(collapsed));
    });
});

// ── Tag filter logic builder ─────────────────────────────────────────────────

function _renderTagRows() {
    const container = $('tag-filter-rows');
    container.innerHTML = '';
    $('tag-filter-empty').style.display = _tagRows.length === 0 ? '' : 'none';

    _tagRows.forEach((row, i) => {
        const div = document.createElement('div');
        // A Paper Tags row is free text, and _evalTagFilter matches it WHOLE
        // against a paper's own tag list, so a typo -- or a tag renamed,
        // merged or deleted elsewhere since the row was added -- matches
        // nothing and filters the canvas down to nothing while reading exactly
        // like a working row. The Projects and Project Tags lists both mark
        // that case (_projectSwatchSlot's "matches no project"); this list was
        // the one that did not, even though `.tag-filter-row.unmatched` -- the
        // style they use to say it -- is written against this row's own class.
        const known = _paperTagSet === null || _paperTagSet.has(_normTag(row.tag));
        div.className = known ? 'tag-filter-row' : 'tag-filter-row unmatched';
        if (!known) div.title = 'No paper on this graph carries this tag.';

        if (i > 0) {
            const op = document.createElement('button');
            op.className = 'tag-op-toggle' + (row.op === 'OR' ? ' or' : '');
            op.textContent = row.op;
            op.title = 'Click to toggle AND / OR';
            op.addEventListener('click', () => {
                _tagRows[i].op = _tagRows[i].op === 'AND' ? 'OR' : 'AND';
                op.textContent = _tagRows[i].op;
                op.className = 'tag-op-toggle' + (_tagRows[i].op === 'OR' ? ' or' : '');
                _applyFilter();
            });
            div.appendChild(op);
        } else {
            // Spacer so labels line up with rows that have an op button
            const sp = document.createElement('span');
            sp.style.cssText = 'min-width:34px; flex-shrink:0;';
            div.appendChild(sp);
        }

        const lbl = document.createElement('span');
        lbl.className = 'tag-filter-label';
        lbl.textContent = row.tag;
        div.appendChild(lbl);

        const rm = document.createElement('button');
        rm.className = 'tag-filter-remove';
        rm.textContent = '×';
        rm.title = 'Remove';
        rm.addEventListener('click', () => {
            _tagRows.splice(i, 1);
            _renderTagRows();
            _applyFilter();
        });
        div.appendChild(rm);

        container.appendChild(div);
    });
}

function _addTag() {
    const input = $('tagFilterInput');
    const tag = input.value.trim();
    if (!tag) return;
    if (_tagRows.some(r => _normTag(r.tag) === _normTag(tag))) { input.value = ''; return; }
    _tagRows.push({ op: 'AND', tag });
    input.value = '';
    _renderTagRows();
    _applyFilter();
}

// The one normalization every paper-tag comparison in this file goes through:
// TRIM, then fold to lower case. Both halves are the backend's own rule rather
// than a convenience.
//
//   - lower case, because TAG.TAG is UNIQUE COLLATE NOCASE, so the datalist
//     offers the canonical TAG-table label while a paper node carries the raw
//     casing from its own metadata: "ML" must match "ml".
//   - trim, because crates/core/src/graph.rs trims a tag (and drops it when
//     nothing is left) before it builds the `tag::<lower>` node id, while it
//     forwards each paper's `tags` array raw -- and nothing upstream guarantees
//     the two agree. `POST /api/papers/{id}/tags` trims, but the archive import
//     path does not: export_import.rs hands the archive's own strings straight
//     to add_paper_tags, which stores them verbatim in PAPER_META.TAGS and
//     creates the TAG row through tag_fk_for_label, which does not trim either.
//
// Folding only the case left that whitespace to split the app in three over one
// tag: the canvas drew a node labelled "ml", `/api/tags` offered "ml " (a
// trailing space is invisible in a dropdown, and NOCASE does not make it a
// duplicate of "ml"), and _addTag trimmed the pick back to "ml" -- which then
// matched no paper. Picking a tag out of the box therefore emptied the graph
// and marked its own row "No paper on this graph carries this tag", on a value
// the box had just supplied, about a chip plainly drawn on the canvas.
function _normTag(t) {
    return String(t).trim().toLowerCase();
}

// The spelling the REST of the app shows for a tag: `TAG.TAG`, as `/api/tags`
// answers with it and as the Tags index, TagPage and this file's own Paper Tags
// dropdown all display it.
//
// A tag NODE is shared by every paper carrying the tag, but `/api/graph` labels
// it from the papers rather than from the TAG table: crates/core/src/graph.rs
// keys the node `tag::<lower>` and keeps the casing of whichever paper it
// happened to reach first (`tag_labels.entry(id).or_insert_with(...)` over
// PAPER_NODES_SQL, which has no ORDER BY). So one tag written "ML" by the Tags
// page and "ml" by an imported archive drew a chip whose spelling depended on
// SQLite's scan order -- it could change under a plain Refresh, after an
// unrelated edit moved the rows -- while the dropdown two panels away offered
// "ML" and TagPage titled itself "ML". Resolve the shared node against the
// shared source of truth instead.
//
// Falls back to the payload's own label whenever `/api/tags` cannot answer for
// it: before the first successful reply, and for a tag that is on a paper but
// not in the list (the reserved reading-list marker, which
// `list_tags_with_count` filters out server-side). Never invents a label.
function _canonicalTagLabel(raw) {
    const trimmed = String(raw == null ? '' : raw).trim();
    if (!_tagLabelByNorm) return trimmed;
    return _tagLabelByNorm.get(_normTag(trimmed)) || trimmed;
}

function _evalTagFilter(paperTags) {
    if (_tagRows.length === 0) return true;
    const tags = (Array.isArray(paperTags) ? paperTags : []).map(_normTag);
    let result = tags.includes(_normTag(_tagRows[0].tag));
    for (let i = 1; i < _tagRows.length; i++) {
        const has = tags.includes(_normTag(_tagRows[i].tag));
        result = _tagRows[i].op === 'AND' ? (result && has) : (result || has);
    }
    return result;
}

$('addTagBtn').addEventListener('click', _addTag);
$('tagFilterInput').addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault(); _addTag(); }
});

$('addProjectBtn').addEventListener('click', () =>
    _addToFilterList('filterProject', _projectFilterNames, _renderProjectRows));
$('filterProject').addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault();
        _addToFilterList('filterProject', _projectFilterNames, _renderProjectRows); }
});

$('addProjTagBtn').addEventListener('click', () =>
    _addToFilterList('filterProjectTag', _projTagFilterNames, _renderProjTagRows));
$('filterProjectTag').addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault();
        _addToFilterList('filterProjectTag', _projTagFilterNames, _renderProjTagRows); }
});

// ── Layout sliders ───────────────────────────────────────────────────────────

// The many-body (repulsion) force for the CURRENT layout membership: -repel for
// the nodes the attribute filters matched, 0 for the ones they excluded, so an
// excluded node exerts nothing on the layout it is no longer part of.
//
// Shared by all three places that install it -- loadGraph (building the
// simulation), filterGraph (the membership changed) and the Repel force slider
// (the magnitude changed) -- because two of them used to build it inline and
// only one of them knew about the membership. Dragging the slider installed a
// flat `strength(-v)`, handing every filtered-out node its full repulsion back
// for as long as the filter stayed on: the ghosts cannot move (filterGraph pins
// them) but they still push, so one drag blew the visible graph apart and no
// filter control had been touched to explain it. The next filter pass silently
// put it back, which is what made it look like the slider itself was unstable.
function _chargeForce() {
    const repel = parseFloat($('repelForce').value);
    const ids = _layoutNodeIds;
    return d3.forceManyBody().strength(
        ids ? (n => (ids.has(n.id) ? -repel : 0)) : -repel);
}

// Per-node collision radius, so the layout holds two node centres 28px apart.
// Node bodies are 20px across (14px for an author diamond), which leaves a
// little room for the label that hangs off each one's right-hand side.
const COLLIDE_RADIUS = 14;

// The overlap-resolving force for the CURRENT layout membership, and the second
// half of the same rule _chargeForce() carries: an excluded node is pinned, not
// removed from the simulation, and d3's collision force reads every node in the
// array whether or not it can move. Zeroing the charge stopped the ghosts
// REPELLING the matching nodes but left them occupying space -- a 28px keep-out
// circle each, still shoving the filtered-down graph around as it collapsed
// towards the origin under the centring force, from behind an 8% ghost with no
// filter control that could explain it.
//
// A zero radius is exactly right rather than merely smaller: d3 splits a
// collision correction between the pair in proportion to the SQUARE of their
// radii, so a member paired with a zero-radius ghost takes 0 of it and the
// ghost takes all of it -- and the ghost is pinned, so its share is discarded
// on the next tick. Member-to-member collisions are untouched, and two ghosts
// stop interacting with each other as well.
function _collideForce() {
    const ids = _layoutNodeIds;
    return d3.forceCollide().radius(
        ids ? (n => (ids.has(n.id) ? COLLIDE_RADIUS : 0)) : COLLIDE_RADIUS);
}

function bindSlider(id, valId, onInput) {
    $(id).addEventListener('input', () => {
        $(valId).textContent = parseFloat($(id).value);
        if (simulation && onInput) onInput(parseFloat($(id).value));
    });
}

bindSlider('centerForce', 'centerForceVal', v => {
    simulation.force('x', d3.forceX(0).strength(v));
    simulation.force('y', d3.forceY(0).strength(v));
    simulation.alpha(0.3).restart();
});
bindSlider('repelForce', 'repelForceVal', () => {
    // Rebuilt through _chargeForce so the new magnitude keeps the current
    // layout membership; the slider's own value is read back off the input.
    simulation.force('charge', _chargeForce());
    simulation.alpha(0.3).restart();
});
bindSlider('linkDistance', 'linkDistanceVal', v => {
    simulation.force('link').distance(v);
    simulation.alpha(0.3).restart();
});
bindSlider('linkStrength', 'linkStrengthVal', v => {
    simulation.force('link').strength(v);
    simulation.alpha(0.3).restart();
});

// "Randomize & restart" throws the settled layout away and rebuilds it from
// fresh seed positions -- exactly the state a cold load is in, and it needs the
// same two things loadGraph() does for that state:
//
//   1. A reframe once the simulation settles. The seeds are an 800x800 square
//      the force layout then spreads well past, and the viewport still frames
//      the OLD layout, so the button left the user on a stale (often
//      near-empty) view of a graph that had moved somewhere else entirely --
//      the one failure the fit-on-settle in loadGraph() exists to prevent.
//   2. The filter's layout pins re-established. Clearing fx/fy below releases
//      the pins filterGraph() owns as well as the drag pins, so every node the
//      attribute filters excluded would start drifting under the centring
//      force -- visible as 8% ghosts sliding across the canvas -- and
//      _filterPinned would be left true against a null fx, so the next filter
//      pass could not repin them either.
$('relayout-btn').addEventListener('click', () => {
    if (!simulation) return;
    // The inspector is placed against a node that is about to jump.
    _hideNodeTooltip();
    const rand = _layoutRng();
    _simNodeById.forEach(n => {
        n.x = (rand() - 0.5) * SEED_SPREAD;
        n.y = (rand() - 0.5) * SEED_SPREAD;
        n.vx = 0; n.vy = 0; n.fx = null; n.fy = null; n._filterPinned = false;
    });
    _fitOnSettle = true;
    // Repins the excluded nodes at their new seed positions and rebuilds the
    // link/charge sets; its own alpha(0.3) restart is superseded by the
    // alpha(1) below, which is what "randomize" asks for.
    _applyFilter();
    simulation.alpha(1).restart();
});

// ── Filter wiring ────────────────────────────────────────────────────────────

const _textFilterIds  = ['filterCategory', 'filterDateFrom', 'filterDateTo',
                         'filterTitle', 'filterAuthor'];

let _projectFilterNames = [];
let _projTagFilterNames = [];
const _checkFilterIds = ['showPapers', 'showAuthors', 'showTags', 'filterHasPdf'];

_textFilterIds.forEach(id => {
    $(id).addEventListener('input', _scheduleFilter);
});
_checkFilterIds.forEach(id => {
    $(id).addEventListener('change', _applyFilter);
});

$('isolate-btn').addEventListener('click', () => {
    $('isolate-btn').classList.toggle('active');
    _applyFilter();
});

$('clear-filters-btn').addEventListener('click', clearFilters);
// The same reset, reachable from the notice that appears when a filter has
// emptied the canvas — the Filters panel is collapsed by default, so the
// existing button is two clicks away at the moment it is needed most.
$('no-match-clear').addEventListener('click', clearFilters);
// The other half of the notice: "Hide single-paper authors" is the HOST's
// checkbox, so the frame can only ask for it back. GraphPage flips the store,
// and its existing option effect posts the `set_options` reload -- the guest
// does not touch _excludeSingleAuthors itself, so the two never disagree.
$('no-match-show-authors').addEventListener('click', () => {
    _notifyHost({ type: 'request_options', excludeSingleAuthors: false });
});

// ── Selection panel buttons ─────────────────────────────────────────────────

$('select-all-btn').addEventListener('click', () => selectAllPapers());
$('clear-selection-btn').addEventListener('click', () => clearSelection());

// How many colour dots one filter row shows before it collapses the rest into
// a "+n". Three keeps the label readable in the panel column's width.
const MAX_ROW_SWATCHES = 3;

// The projects one Projects-filter row resolves to. Case-insensitive SUBSTRING,
// which is what _projectIdsFromInput turns into the id list filterGraph
// receives -- so the dots drawn on a row are exactly what the canvas is being
// filtered by, not a second opinion about it.
function _projectsMatchingName(name) {
    const lower = name.toLowerCase();
    return [..._projectMap.values()].filter(p => String(p.name).toLowerCase().includes(lower));
}

// The projects one Project Tags row resolves to. filterGraph compares project
// tags case-insensitively and WHOLE (`projTagSet.has`), never by substring, so
// this has to fold the same way rather than reuse the match above.
function _projectsWithTag(tag) {
    const lower = tag.toLowerCase();
    return [..._projectMap.values()].filter(p =>
        Array.isArray(p.tags) && p.tags.some(t => String(t).toLowerCase() === lower));
}

// The leading slot of a project / project-tag filter row, in the same 34px
// column the paper-tag rows give their AND/OR toggle: one colour dot per
// project the row resolves to.
//
// `/api/graph/project-options` has always carried a `color` per project
// (graph.rs defaults it to the accent, so it is never absent) and no code path
// read it -- the graph's Projects filter was the one surface in the app that
// showed a project without the swatch ProjectCard / ProjectDetailPage /
// ProjectsPage draw beside it.
//
// The row that resolves to NOTHING is the case worth seeing: the rows are free
// text, so a typo -- or a project renamed or deleted elsewhere since the row
// was added -- makes _projectIdsFromInput fall through to its `[-1]` sentinel
// and filters the canvas down to nothing, while the row that did it reads
// exactly like a working one.
function _projectSwatchSlot(projects) {
    const slot = document.createElement('span');
    if (projects.length === 0) {
        slot.className = 'proj-swatches none';
        slot.textContent = '\u2717';
        slot.title = 'Matches no project';
        return slot;
    }
    slot.className = 'proj-swatches';
    projects.slice(0, MAX_ROW_SWATCHES).forEach(p => {
        const dot = document.createElement('span');
        dot.className = 'proj-swatch';
        // A style-property write, never markup: the colour is backend text.
        dot.style.backgroundColor = p.color || 'var(--color-accent, #5b8dee)';
        slot.appendChild(dot);
    });
    if (projects.length > MAX_ROW_SWATCHES) {
        const more = document.createElement('span');
        more.className = 'proj-swatch-more';
        more.textContent = '+' + (projects.length - MAX_ROW_SWATCHES);
        slot.appendChild(more);
    }
    slot.title = projects.map(p => p.name).join(', ');
    return slot;
}

// Whether a project the user is filtering by has any paper on THIS canvas.
//
// `/api/graph/project-options` answers with every ACTIVE project, while
// filterGraph matches a paper through its own `project_ids` -- so a project
// that holds no paper the payload drew (a project just created and still
// empty, or one whose papers have all been deleted or superseded) resolves
// perfectly and still matches nothing. Null before the first load: a graph
// that has not arrived cannot be used to claim a project is absent from it.
function _projectHasDrawnPaper(proj) {
    return _paperProjectIds === null || _paperProjectIds.has(proj.id);
}

function _projectsDrawingPapers(projects) {
    return projects.filter(_projectHasDrawnPaper);
}

// `resolve` maps a row's free text to the projects it currently stands for,
// `rerender` redraws the list after a removal and `noPapersTitle` says how this
// list phrases "the projects are real, the papers are not here" -- all three
// list-specific, so the two wrappers below own them and every other caller goes
// through those.
//
// A row is marked `unmatched` for either of the two ways it can stand for
// nothing, because the canvas cannot tell them apart: the free text resolves to
// no project at all (the swatch slot's own "✗"), or it resolves to projects
// none of whose papers are on this graph. The Paper Tags list has drawn that
// second line since it started checking rows against _paperTagSet -- its rows
// are marked against the CANVAS, not against `/api/tags` -- and these two
// sibling lists were still marking against the options payload alone.
function _renderFilterList(rows, containerId, emptyId, resolve, rerender, noPapersTitle) {
    const container = $(containerId);
    container.innerHTML = '';
    $(emptyId).style.display = rows.length === 0 ? '' : 'none';
    rows.forEach((name, i) => {
        const div = document.createElement('div');
        const matched = resolve(name);
        const drawing = _projectsDrawingPapers(matched);
        div.className = drawing.length === 0 ? 'tag-filter-row unmatched' : 'tag-filter-row';
        // The swatches stay: which projects the row resolves to is a true fact
        // about it, and it is the only thing that tells the two cases apart.
        // Only the second one needs saying in words.
        if (matched.length > 0 && drawing.length === 0) div.title = noPapersTitle;
        div.appendChild(_projectSwatchSlot(matched));
        const lbl = document.createElement('span');
        lbl.className = 'tag-filter-label';
        lbl.textContent = name;
        div.appendChild(lbl);
        const rm = document.createElement('button');
        rm.className = 'tag-filter-remove';
        rm.textContent = '×';
        rm.title = 'Remove';
        rm.addEventListener('click', () => {
            rows.splice(i, 1);
            rerender();
            _applyFilter();
        });
        div.appendChild(rm);
        container.appendChild(div);
    });
}

function _renderProjectRows() {
    _renderFilterList(_projectFilterNames, 'project-filter-rows', 'project-filter-empty',
                      _projectsMatchingName, _renderProjectRows,
                      'No paper on this graph belongs to that project.');
}

function _renderProjTagRows() {
    _renderFilterList(_projTagFilterNames, 'proj-tag-filter-rows', 'proj-tag-filter-empty',
                      _projectsWithTag, _renderProjTagRows,
                      'No paper on this graph belongs to a project with this tag.');
}

function _addToFilterList(inputId, rows, rerender) {
    const input = $(inputId);
    const val = input.value.trim();
    // Both lists match case-insensitively downstream (project names by
    // substring, project tags against NOCASE tag labels), so "ML" after "ml"
    // would add a second row that filters identically.
    const dup = rows.some(r => r.toLowerCase() === val.toLowerCase());
    if (!val || dup) { input.value = ''; return; }
    rows.push(val);
    input.value = '';
    rerender();
    _applyFilter();
}

function _projectIdsFromInput() {
    if (_projectFilterNames.length === 0) return null;
    const ids = [];
    _projectFilterNames.forEach(name => {
        _projectsMatchingName(name).forEach(p => ids.push(p.id));
    });
    return ids.length > 0 ? ids : [-1];
}

function _projTagFromInput() {
    return _projTagFilterNames.length > 0 ? _projTagFilterNames : null;
}

function _scheduleFilter() {
    clearTimeout(_debounce);
    _debounce = setTimeout(_applyFilter, 280);
}

// ── Active-filter badges ─────────────────────────────────────────────────────
// Filters and Tag Filter both open COLLAPSED (graph.html), and everything in
// them lives as long as the frame does: AppShell's keep-alive holds this iframe
// across every route change, so a category typed once, a tag row added once or
// a Visibility box unchecked once survives navigating away, coming back, a
// Refresh and an option toggle. Nothing on either collapsed header said so.
//
// What the user is then looking at is a field of 8% ghosts -- which is exactly
// what an active filter is SUPPOSED to look like -- with the control that
// caused it, and the "Clear all filters" button that undoes it, two clicks away
// behind a "▶". The one surface that does explain a filter, the no-match
// notice, appears only once NOTHING is drawn at all, so the whole partially
// filtered range -- the common case -- had no cue anywhere. Report what is on
// in the header, the same slot the Selection panel already uses for its count.

// The Filters panel's active controls, in the order the panel lists them. The
// three Visibility checkboxes count: "don't draw authors" is not an attribute
// filter, but it is still a reason the canvas is not showing what the library
// holds, and it is just as invisible from a collapsed header.
function _activeFilterSummary() {
    const on = [];
    if (!$('showPapers').checked)  on.push('Papers hidden');
    if (!$('showAuthors').checked) on.push('Authors hidden');
    if (!$('showTags').checked)    on.push('Tags hidden');
    const category = $('filterCategory').value.trim();
    if (category) on.push('Category: ' + category);
    if ($('filterHasPdf').checked) on.push('Has PDF only');
    const from = $('filterDateFrom').value.trim();
    if (from) on.push('Published from ' + from);
    const to = $('filterDateTo').value.trim();
    if (to) on.push('Published to ' + to);
    const title = $('filterTitle').value.trim();
    if (title) on.push('Title: ' + title);
    const author = $('filterAuthor').value.trim();
    if (author) on.push('Author: ' + author);
    if ($('isolate-btn').classList.contains('active')) on.push('Show highlighted only');
    return on;
}

// The Tag Filter panel's three lists. Only the ROWS count: text left sitting in
// an add-box is not a filter (nothing reads it until "+"/Enter), which is the
// same line _applyFilter itself draws. The paper-tag rows carry the AND/OR they
// are combined with, since two rows joined by OR filter nothing like the same
// two joined by AND.
function _activeTagFilterSummary() {
    return [
        ..._projectFilterNames.map(n => 'Project: ' + n),
        ..._projTagFilterNames.map(n => 'Project tag: ' + n),
        ..._tagRows.map((r, i) => (i > 0 ? r.op + ' ' : '') + 'Tag: ' + r.tag),
    ];
}

// A leading space rides in the text rather than in graph.html, so a panel with
// nothing active reads exactly as it did before this existed. The tooltip goes
// on the badge, not the header, for the same reason: it should not exist while
// there is nothing to name.
function _paintPanelCount(id, items) {
    const el = $(id);
    if (!el) return;
    el.textContent = items.length ? ' (' + items.length + ')' : '';
    el.title = items.join('\n');
}

function _updateFilterBadges() {
    _paintPanelCount('filter-active-count', _activeFilterSummary());
    _paintPanelCount('tag-filter-active-count', _activeTagFilterSummary());
}

function _applyFilter() {
    // Every filter change in this document funnels through here -- the text
    // boxes via _scheduleFilter, the checkboxes and the isolate button
    // directly, each row list after an add/remove/AND-OR toggle, clearFilters,
    // and loadGraph once a payload is in -- so this is the one place the
    // badges have to be refreshed from.
    _updateFilterBadges();
    filterGraph({
        showPapers:   $('showPapers').checked,
        showAuthors:  $('showAuthors').checked,
        showTags:     $('showTags').checked,
        category:     $('filterCategory').value.trim() || null,
        hasPdf:       $('filterHasPdf').checked,
        dateFrom:     $('filterDateFrom').value.trim() || null,
        dateTo:       $('filterDateTo').value.trim()   || null,
        highlight:    $('filterTitle').value.trim()    || null,
        authorFilter: $('filterAuthor').value.trim()   || null,
        isolate:      $('isolate-btn').classList.contains('active'),
        projectIds:   _projectIdsFromInput(),
        projTagIds:   _projTagFromInput(),
    });
}

// ── Filter datalists, populated from the fetched dropdown payloads ───────────

function _escAttr(s) {
    return String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/'/g, '&#39;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// The reserved marker that MAKES a project a reading list -- the same string as
// src/lib/readingStatus.ts's READING_LIST_TAG and
// crates/core/src/storage/queries/tag.rs's, which keeps it out of `/api/tags`
// for exactly this reason. It is bookkeeping rather than something anyone
// typed: ProjectDetailPage refuses to let it be added by hand, and every
// surface that DRAWS a project's tags filters it back out (the chip row on
// ProjectDetailPage, the ProjectCard chips on ReadingListsPage).
//
// `/api/graph/project-options` forwards each project's project_tags raw, so
// this datalist was the one dropdown left in the app offering it: any user with
// a reading list found a tag in the graph's Project Tags box that they never
// created, that names an implementation detail, and that exists nowhere else in
// the interface. Pinned equal to the host's copy in graphIframeAssets.test.ts.
const READING_LIST_TAG = 'reading-list';

// The Paper Tags datalist: the tags `/api/tags` knows about, narrowed to the ones
// a paper on THIS canvas actually carries.
//
// `/api/tags` is `list_tags_with_count` (crates/core/src/storage/queries/tag.rs),
// which walks the whole TAG table by LEFT JOIN and keeps the rows that join to
// nothing -- its own test pins a tag with `paper_count: 0` in the answer.
// Projects share that table (add_project_tags inserts into TAG) and
// remove_paper_tags leaves the row behind when a paper drops its last link, so
// the answer routinely holds labels no paper carries. _evalTagFilter matches a
// row against a PAPER's own tag list, so every one of those was a dropdown entry
// whose only possible effect was to empty the canvas -- and, since the unmatched
// marking, to draw itself as "matches nothing" the instant it was added, on a
// value the list had just offered. The Projects and Project Tags lists each
// offer exactly the universe they filter (project names / project tags, both off
// `/api/graph/project-options`); this was the one list offering from a wider one.
//
// Narrowed against the CANVAS rather than against the `paper_count` the payload
// also carries: that count is over every version of an active paper, while
// PAPER_NODES_SQL draws only the latest, so a tag left on an older version alone
// counts above zero and still matches nothing here. _paperTagSet is the set
// _evalTagFilter itself reads.
//
// Only the OFFER is narrowed, the same split setFilterOptions makes for the
// reading-list marker: a tag typed by hand still reaches _evalTagFilter, still
// filters, and is still marked unmatched when it stands for nothing.
function _renderTagDatalist() {
    if (!_tagLabelByNorm) return;
    // _tagLabelByNorm already holds the form a ROW holds -- _addTag trims what
    // it is given -- deduped on _normTag, because TAG.TAG's UNIQUE COLLATE
    // NOCASE stops "ml" and "ML" coexisting but not "ml" and "ml ". Offering
    // the raw label meant the box could suggest a spelling no row it builds can
    // ever have. Before the first payload there is nothing to narrow against,
    // and claiming a tag is absent from a graph that has not loaded would be a
    // guess.
    const offered = [];
    _tagLabelByNorm.forEach((label, norm) => {
        if (_paperTagSet && !_paperTagSet.has(norm)) return;
        offered.push(label);
    });
    $('tagList').innerHTML = offered.map(t => `<option value="${_escAttr(t)}">`).join('');
}

// The three lists are independent, and each one is optional: `null` means "that
// payload never arrived, keep what the last successful load installed" rather
// than blanking a datalist because one auxiliary endpoint was down (see
// fetchAndLoadGraph). An empty ARRAY is a real answer -- a library with no
// papers has no categories -- and does clear the list.
function setFilterOptions(categories, tags, projects) {
    if (categories) {
        $('categoryList').innerHTML = categories.map(c => `<option value="${_escAttr(c)}">`).join('');
    }
    if (tags) {
        // Indexed once, here, rather than re-walked by each reader: the offer
        // and the canvas chips have to agree on which spelling of a tag is the
        // canonical one, and the first entry wins for both.
        _tagLabelByNorm = new Map();
        tags.forEach(raw => {
            const norm = _normTag(raw);
            if (!norm || _tagLabelByNorm.has(norm)) return;
            _tagLabelByNorm.set(norm, String(raw).trim());
        });
        _renderTagDatalist();
        // The tag NODES are labelled from this map too (_canonicalTagLabel),
        // and nothing relabels the ones already drawn -- it does not have to:
        // fetchAndLoadGraph installs the options and then calls loadGraph in
        // the same pass, so every answer this map is built from is followed by
        // the load that reads it.
    }
    if (!projects) return;

    _projectMap.clear();
    projects.forEach(proj => { _projectMap.set(proj.id, proj); });
    _renderProjectDatalists();
    // The rows themselves are free text the user typed, but what they RESOLVE
    // to is not, and it just changed: a project renamed, recoloured, deleted or
    // retagged elsewhere has to repaint the swatches -- and the "matches no
    // project" mark -- on rows drawn against the previous load's options.
    _renderProjectRows();
    _renderProjTagRows();
}

// The Projects and Project Tags dropdowns: the active projects `/api/graph/
// project-options` knows about, narrowed to the ones a paper on THIS canvas
// belongs to -- exactly the narrowing _renderTagDatalist makes for Paper Tags,
// and for exactly the same reason. Both boxes filter PAPERS, and a paper is
// matched through its own `project_ids`, so an active project with no paper on
// the graph (one just created and still empty, or one whose papers have been
// deleted or superseded) was a dropdown entry whose only possible effect was to
// empty the canvas -- and, since the row marking above, to draw itself as
// standing for nothing the instant it was added, on a value the list had just
// offered.
//
// Rebuilt from BOTH sides for the same reason the tag list is: from
// setFilterOptions when the project payload changes, and from loadGraph when
// the PAPERS change, so a project that lost its last paper on this graph stops
// being suggested rather than waiting for the options endpoint to disagree
// (it never will -- the PROJECT row outlives the paper link).
//
// Only the OFFER is narrowed, the same split the reading-list marker gets:
// _projectMap keeps every project and every tag, so a name typed by hand still
// resolves, still filters, and is still marked when it stands for nothing.
function _renderProjectDatalists() {
    const offered = [..._projectMap.values()].filter(_projectHasDrawnPaper);
    $('projectList').innerHTML = offered
        .map(p => `<option value="${_escAttr(p.name)}">`)
        .join('');
    const allProjTags = new Set();
    offered.forEach(proj => {
        (proj.tags || []).forEach(t => {
            if (String(t).toLowerCase() !== READING_LIST_TAG) allProjTags.add(t);
        });
    });
    $('projectTagList').innerHTML = [...allProjTags].sort()
        .map(t => `<option value="${_escAttr(t)}">`)
        .join('');
}

// ── Reset every filter panel to its default and re-apply ─────────────────────

function clearFilters() {
    _textFilterIds.forEach(id => { $(id).value = ''; });
    // All three add-boxes, not two: un-added text is not a filter, but leaving
    // one of three siblings holding a half-typed tag after "Clear all filters"
    // is the panel disagreeing with its own button.
    $('filterProject').value = '';
    $('filterProjectTag').value = '';
    $('tagFilterInput').value = '';
    _checkFilterIds.forEach(id => { $(id).checked = id !== 'filterHasPdf'; });
    $('isolate-btn').classList.remove('active');
    _tagRows.length = 0;
    _renderTagRows();
    _projectFilterNames.length = 0;
    _renderProjectRows();
    _projTagFilterNames.length = 0;
    _renderProjTagRows();
    _applyFilter();
}

// ── Graph loading ─────────────────────────────────────────────────────────────

// Fit with a margin. cytoscape's getFitViewport() bails silently unless the
// bounding box AND the container both have a positive width and height, so a
// fit asked for at the wrong moment is not an error — it just never happens,
// and the graph stays at the default zoom 1 / pan 0,0 with the layout spread
// off-screen around the origin. Both cases are handled here rather than left
// to that silent bail:
//   - empty graph: nothing to frame, and the next load re-fits anyway;
//   - zero-sized viewport: the host keeps this iframe alive behind
//     `display: none` while the user is on another page (AppShell's
//     keep-alive) and #cy is sized in vw/vh, so a load or a settle that lands
//     while hidden would lose its framing for good. Remember it instead and
//     let the resize the reveal fires replay it.
//
// cy.fit() also frames across the WHOLE canvas, but #right-panels is
// `position: fixed` over its right edge, so a plain fit pushes the rightmost
// nodes — and their right-hand labels, which stick out further still —
// underneath the filter panels on every load, settle and reveal. Frame into
// the strip the panels leave uncovered instead.
const FIT_PADDING = 40;

// Width of the canvas the panel column covers, measured from the column's own
// box so it tracks graph.css rather than a second copy of its numbers. #cy is
// 100vw/100vh on a margin-0 body, so the column's viewport-relative left edge
// is already a canvas-relative one. Zero when the column has no layout
// (jsdom-free tests, standalone loads with the panels removed), which is the
// honest answer: nothing is covered.
function _panelGutter(canvasWidth) {
    const panels = $('right-panels');
    if (!panels || !panels.getBoundingClientRect) return 0;
    const rect = panels.getBoundingClientRect();
    if (!(rect.width > 0)) return 0;
    return Math.max(0, canvasWidth - rect.left);
}

// The nodes a fit should frame: the ones _applyAllStyles leaves at a NON-ZERO
// opacity, i.e. the ones the user can actually see. `null` means "nothing is
// being held back, frame the whole graph" -- which is both the cold-load answer
// and the reason an unfiltered fit is byte-identical to what it was before this
// existed.
//
// _fitGraph() used to frame `cy.elements()`, the extent of everything the
// payload HOLDS, which is not the same set once a filter or a Visibility
// checkbox is in force. "Randomize & restart" is where that shows: it throws
// every node -- ghosts included -- back into the 800x800 seed box and re-arms
// the fit-on-settle, but the filter state survives it (nothing in this document
// clears a filter on relayout). So with "Show highlighted only" on and three
// papers matching, the settle framed the spread of the whole library while the
// three visible nodes collapsed into a speck in the middle of it; with
// Visibility > Authors off, it framed the invisible authors' ring around a
// canvas drawing only papers and tags. The same applies to the fit the reveal
// replays (_fitDeferred), which runs against whatever filter state the panel
// was left in.
//
// The rule is exactly _applyAllStyles's own: a node of a hidden TYPE is not
// drawn at all, and under isolate a node the attribute filters excluded is
// taken to opacity 0. An ordinary 8% ghost IS drawn -- faintly, deliberately --
// so it stays inside the frame, and the non-isolate case is unchanged.
function _drawnNodes() {
    if (!cy) return null;
    const isolating = _filterIsolate && _visiblePaperIds !== null;
    // Nothing excluded: every node is drawn, so there is nothing to narrow to
    // and cy.elements() -- which also carries the edges -- stays the answer.
    if (_hiddenTypes.size === 0 && !isolating) return null;
    const matched = {
        paper:  _visiblePaperIds,
        author: _visibleAuthorIds,
        tag:    _visibleTagIds,
    };
    const drawn = cy.nodes().filter(n => {
        const type = n.data('type');
        if (_hiddenTypes.has(type)) return false;
        if (!isolating) return true;
        const set = matched[type];
        return !!set && set.has(n.id());
    });
    // Isolate with a filter matching nothing draws an empty canvas; there is no
    // extent to frame, so fall back to the whole graph rather than to a
    // degenerate box. The no-match notice is what explains that state.
    return drawn.length > 0 ? drawn : null;
}

function _fitGraph() {
    if (!cy || cy.nodes().length === 0) return;
    const w = cy.width(), h = cy.height();
    if (!w || !h) { _fitDeferred = true; return; }
    _fitDeferred = false;

    // Passed to every fallback below as well: cytoscape's own cy.fit() takes
    // the collection to frame, so the narrowed set survives the degradations
    // rather than only applying on the happy path.
    const framed = _drawnNodes();
    const gutter = _panelGutter(w);
    const avail  = w - gutter;
    if (gutter <= 0 || avail <= 2 * FIT_PADDING) { cy.fit(framed || undefined, FIT_PADDING); return; }

    const bb = (framed || cy.elements()).boundingBox();
    if (!(bb.w > 0) || !(bb.h > 0)) { cy.fit(framed || undefined, FIT_PADDING); return; }

    // cytoscape's own getFitViewport() math, with the width narrowed to the
    // strip the panels leave uncovered: zoom to the tighter of the two axes,
    // then centre the bounding box inside that strip.
    let zoom = Math.min((avail - 2 * FIT_PADDING) / bb.w, (h - 2 * FIT_PADDING) / bb.h);
    zoom = Math.max(Math.min(zoom, cy.maxZoom()), cy.minZoom());  // minZoom wins, as cytoscape does
    if (!(zoom > 0)) { cy.fit(framed || undefined, FIT_PADDING); return; }
    cy.viewport({
        zoom: zoom,
        pan: {
            x: (avail - zoom * (bb.x1 + bb.x2)) / 2,
            y: (h     - zoom * (bb.y1 + bb.y2)) / 2,
        },
    });
}

// ── Hover inspector ────────────────────────────────────────────────────────
// Paper labels are drawn with `text-max-width: 180px` + `text-wrap: ellipsis`,
// so any real title is cut off on the canvas and the only way to read it was to
// click through and leave the graph. Meanwhile `/api/graph` already sends the
// category, published date, tags, has_pdf flag and the whole abstract for every
// paper (crates/core/src/graph.rs's PAPER_NODES_SQL selects summary, url and
// doi as well) -- loadGraph copied all of it into cytoscape's node data and
// nothing ever read a byte of it. Surface it on hover instead: it is the same
// "peek without navigating" affordance the rest of the app gives a paper row.
const TOOLTIP_SUMMARY_MAX = 260;   // chars of abstract before the ellipsis
const TOOLTIP_OFFSET      = 14;    // gap between the cursor/node and the box
const TOOLTIP_MARGIN      = 8;     // keep-inside-the-viewport margin

function _truncate(text, max) {
    const t = String(text).replace(/\s+/g, ' ').trim();
    if (t.length <= max) return t;
    const cut = t.slice(0, max);
    const sp  = cut.lastIndexOf(' ');
    return (sp > max * 0.6 ? cut.slice(0, sp) : cut).trimEnd() + '\u2026';
}

function _pluralPapers(n) {
    return n + (n === 1 ? ' paper' : ' papers');
}

// Papers an author or tag node is joined to, split into the whole degree and
// the part of it the current filter state actually DRAWS. The edge list is the
// only place either number exists client-side -- /api/graph sends no degree.
//
// `drawn` tracks exactly what _applyAllStyles paints a paper at full opacity
// for: the Visibility checkbox switches the whole type off, and an attribute
// filter leaves the non-matching ones as 8% ghosts. It is deliberately NOT
// _visibleAuthorIds/_visibleTagIds -- those say whether the hovered node itself
// is drawn, which the user can see; this says how much of what it stands for is.
function _connectedPaperCounts(node) {
    const papersHidden = _hiddenTypes.has('paper');
    let total = 0, drawn = 0;
    node.connectedEdges().forEach(e => {
        const other = e.source().id() === node.id() ? e.target() : e.source();
        if (other.data('type') !== 'paper') return;
        total++;
        if (papersHidden) return;
        if (_visiblePaperIds === null || _visiblePaperIds.has(other.id())) drawn++;
    });
    return { total, drawn };
}

// "Author \u00b7 37 papers", plus what the filter left of those 37 when it is
// not all of them.
//
// The degree is a fact about the library -- AuthorsPage reports the same number
// -- so filtering the canvas must not silently rewrite it; but a node hovered
// on a filtered graph is standing in for a set the canvas is mostly not
// showing, and reading "37 papers" off a node with one line leaving it is the
// same disagreement the Selection counter ("(5, 2 hidden)") and the collapsed
// panel badges already had to close. Report both rather than pick one:
// with no filter in force the two are equal and the line is unchanged.
function _degreeLine(kind, node) {
    const { total, drawn } = _connectedPaperCounts(node);
    const head = kind + ' \u00b7 ' + _pluralPapers(total);
    if (drawn === total) return head;
    return head + (drawn === 0 ? ' (none shown)' : ' (' + drawn + ' shown)');
}

// The meta lines under the title, joined with newlines and rendered by
// graph.css's `white-space: pre-line` -- set through textContent, never
// innerHTML, because every string here is library metadata.
function _tooltipMetaFor(node) {
    const d = node.data();
    const lines = [];
    if (d.type === 'paper') {
        const head = [];
        if (d.category) head.push(d.category);
        // `published` is already folded to null for the 0001-01-01 sentinel, so
        // "no date" is sayable rather than a bogus year 1.
        head.push(d.published || 'No publication date');
        head.push(d.has_pdf ? 'PDF' : 'No PDF');
        lines.push(head.join(' \u00b7 '));
        // The tag nodes this paper contributes, not the raw JSON column:
        // graph.rs trims each tag, drops the ones left empty and emits ONE node
        // per `tag::<lower>`, so an untrimmed or case-variant duplicate in
        // PAPER_META.TAGS listed a chip twice -- or a bare separator for a chip
        // the canvas never drew at all. Spelled the way the chip beside it is
        // (_canonicalTagLabel), for the same reason: this line names what the
        // canvas drew, and the paper's own casing is not what it drew.
        const tagChips = [];
        const seenTags = new Set();
        (Array.isArray(d.tags) ? d.tags : []).forEach(t => {
            const norm = _normTag(t);
            if (!norm || seenTags.has(norm)) return;
            seenTags.add(norm);
            tagChips.push(_canonicalTagLabel(t));
        });
        if (tagChips.length) lines.push(tagChips.join(' \u00b7 '));
        if (d.summary) lines.push(_truncate(d.summary, TOOLTIP_SUMMARY_MAX));
    } else if (d.type === 'author') {
        lines.push(_degreeLine('Author', node));
    } else if (d.type === 'tag') {
        lines.push(_degreeLine('Tag', node));
    }
    return lines.join('\n');
}

// Place the box near the node but inside the canvas, and clear of the fixed
// #right-panels column -- the same gutter _fitGraph() frames around.
function _positionTooltip(tip, rendered) {
    // Measure from the top-left first. The box is `position: fixed` with a
    // max-width, so its shrink-to-fit width depends on how much room is left of
    // the viewport edge -- measuring it where the LAST hover parked it would
    // read a width squeezed by that position and then flip on a stale number.
    tip.style.left = '0px';
    tip.style.top  = '0px';
    const box    = tip.getBoundingClientRect ? tip.getBoundingClientRect() : null;
    const w      = (box && box.width)  || 0;
    const h      = (box && box.height) || 0;
    const vw     = window.innerWidth  || 0;
    const vh     = window.innerHeight || 0;
    const rightLimit = (vw - _panelGutter(vw)) - TOOLTIP_MARGIN;

    let x = rendered.x + TOOLTIP_OFFSET;
    let y = rendered.y + TOOLTIP_OFFSET;
    if (vw && x + w > rightLimit) x = rendered.x - TOOLTIP_OFFSET - w;
    if (vh && y + h > vh - TOOLTIP_MARGIN) y = rendered.y - TOOLTIP_OFFSET - h;
    tip.style.left = Math.max(TOOLTIP_MARGIN, x) + 'px';
    tip.style.top  = Math.max(TOOLTIP_MARGIN, y) + 'px';
}

function _showNodeTooltip(node) {
    const tip = $('node-tooltip');
    if (!tip) return;
    const label = node.data('label');
    $('node-tooltip-title').textContent = label ? String(label) : '(untitled)';
    $('node-tooltip-meta').textContent  = _tooltipMetaFor(node);
    tip.style.display = '';
    tip.setAttribute('aria-hidden', 'false');
    // Measured after the content is in and the box is displayed, so the
    // flip-left/flip-up decisions use this tooltip's real size.
    _positionTooltip(tip, node.renderedPosition());
    const canvas = $('cy');
    if (canvas) canvas.classList.add('node-hover');
}

// Hiding is unconditional and cheap, so every path that can strand the box --
// pointer leaving the node, a pan/zoom, a drag, a click that navigates, a
// reload that destroys the graph -- can just call it.
function _hideNodeTooltip() {
    const tip = $('node-tooltip');
    if (tip) {
        tip.style.display = 'none';
        tip.setAttribute('aria-hidden', 'true');
    }
    const canvas = $('cy');
    if (canvas) canvas.classList.remove('node-hover');
}

// PAPER_META.PUBLISHED stores chrono's `date.min` for a paper with no
// publication date, and `/api/graph` (crates/core/src/graph.rs) forwards the
// column raw -- unlike every other serializer, where models.rs blanks it
// (SearchResultOut) or hands back a NULL date (PaperDetails). Left as-is it
// reads as a real date in year 1, so the Date range filter's `From` box
// silently dropped every undated paper off the canvas as "too old", and the
// user had no way to tell a missing date from an excluded one. Fold it to the
// same "no date" the filter already handles: an undated paper is not filtered
// BY date. Same sentinel and the same reasoning as
// storage/queries/paper.rs's NO_PUBLISHED_DATE, which keeps undated papers
// last under an ascending sort rather than first.
const NO_PUBLISHED_DATE = '0001-01-01';
function _publishedOrNull(v) {
    return v && v !== NO_PUBLISHED_DATE ? v : null;
}

// ── Seed positions for an in-place reload ────────────────────────────
// The square of random start positions a cold load hands d3, centred on the
// world origin the centring force pulls towards.
const SEED_SPREAD = 800;
// Spread around a neighbour centroid. Nodes seeded at the exact same point give
// the repulsion no direction to push them apart, and papers imported together
// share their whole neighbourhood.
const SEED_JITTER = 40;
// Rounds of "place every node that now has a placed neighbour". Two is what an
// imported paper needs: the first puts the PAPER beside the authors and tags it
// shares with the existing library, the second puts its brand-new author nodes
// beside the paper. Anything still unreached is an entirely new island and
// keeps its box seed, which is the honest answer -- there is nothing to sit by.
const SEED_PASSES = 2;

// A refresh or a "hide single-paper authors" toggle reloads through
// loadGraph({preserveView: true}), which seeds every surviving node from the
// settled layout and deliberately holds the current zoom/pan. A node that is
// NEW to that load has no previous position, and the cold-load answer -- a
// random point in an 800x800 box at the origin -- is the wrong one for it
// twice over. The force layout normally spreads the graph far wider than that
// box, so a paper imported elsewhere in the app appears nowhere near the
// authors and tags it is joined to and then travels the whole way across the
// canvas under the link force; and every new node arriving in the same clump at
// the centre shoves that settled neighbourhood apart, under a viewport that is
// deliberately not reframing to show where anything went. Seed each new node at
// the centroid of the neighbours that DO have a position instead.
function _seedNewNodesFromNeighbours(simNodes, placedIds, rand) {
    if (placedIds.size === 0 || placedIds.size === simNodes.length) return;
    const byId = new Map(simNodes.map(n => [n.id, n]));
    // Undirected adjacency over the edges of THIS payload. /api/graph emits
    // paper→author and paper→tag, so a new paper reaches its neighbours through
    // the source side and a new author through the target side.
    const neighbours = new Map();
    const link = (a, b) => {
        const list = neighbours.get(a);
        if (list) list.push(b); else neighbours.set(a, [b]);
    };
    _allEdgeDefs.forEach(e => {
        if (!byId.has(e.source) || !byId.has(e.target)) return;
        link(e.source, e.target);
        link(e.target, e.source);
    });

    for (let pass = 0; pass < SEED_PASSES; pass++) {
        // Collected and applied per pass rather than as we go, so a node placed
        // in this pass cannot seed another one in the same pass -- the result
        // would then depend on the order the payload happened to list nodes in.
        const seeded = [];
        simNodes.forEach(n => {
            if (placedIds.has(n.id)) return;
            let sx = 0, sy = 0, count = 0;
            (neighbours.get(n.id) || []).forEach(id => {
                if (!placedIds.has(id)) return;
                const nb = byId.get(id);
                sx += nb.x; sy += nb.y; count++;
            });
            if (count === 0) return;
            n.x = sx / count + (rand() - 0.5) * SEED_JITTER;
            n.y = sy / count + (rand() - 0.5) * SEED_JITTER;
            seeded.push(n.id);
        });
        if (seeded.length === 0) return;
        seeded.forEach(id => placedIds.add(id));
    }
}

function loadGraph(data, opts = {}) {
    const { nodes, edges } = data;
    const { preserveView = false } = opts;

    // The fit below frames the RANDOM seed positions; the force layout then
    // spreads the graph well past them, so a fresh load must reframe once the
    // simulation settles or the user is left staring at a near-empty canvas.
    // Cleared on the first grab so we never reframe under an in-progress drag.
    // Module-level (see _fitOnSettle) because "Randomize & restart" re-arms it
    // from outside this function.
    _fitOnSettle = !preserveView;

    // Seed surviving nodes from the outgoing layout and hold zoom/pan so an
    // in-place reload starts from the current view instead of re-randomising.
    const prevPositions = new Map();
    let prevZoom = null, prevPan = null;
    if (preserveView) {
        _simNodeById.forEach((sn, id) => prevPositions.set(id, { x: sn.x, y: sn.y }));
        if (cy) { prevZoom = cy.zoom(); prevPan = { ...cy.pan() }; }
    }

    if (simulation) { simulation.stop(); simulation = null; }
    if (cy) { cy.destroy(); cy = null; }
    // The box is positioned against a node that is about to stop existing.
    _hideNodeTooltip();
    _fitDeferred = false;
    _simNodeById = new Map();
    _visiblePaperIds  = null;
    _visibleAuthorIds = null;
    _visibleTagIds    = null;
    _hiddenTypes      = new Set();
    // The outgoing graph's ids mean nothing to the incoming one; the
    // _applyFilter() at the end of this function fills it in again.
    _layoutNodeIds    = null;

    // Store original edge defs before D3 mutates source/target to object refs.
    // Built before the seed positions below, which read this adjacency.
    _allEdgeDefs = edges.map(e => ({ source: String(e.source), target: String(e.target) }));

    const rand = _layoutRng();
    // Ids that arrived with a position from the outgoing layout. Empty on a
    // cold load, which is what keeps _seedNewNodesFromNeighbours() a no-op (and
    // the LAYOUT_SEED sequence identical) there.
    const placedIds = new Set();
    const simNodes = nodes.map(n => {
        const id = String(n.id);
        const prev = prevPositions.get(id);
        if (prev) placedIds.add(id);
        return {
            id: id,
            x:  prev ? prev.x : (rand() - 0.5) * SEED_SPREAD,
            y:  prev ? prev.y : (rand() - 0.5) * SEED_SPREAD,
        };
    });
    _seedNewNodesFromNeighbours(simNodes, placedIds, rand);
    const simLinks = _allEdgeDefs.map(e => ({ ...e }));
    simNodes.forEach(n => _simNodeById.set(n.id, n));

    // Index every tag this payload's papers carry, normalized exactly as
    // _evalTagFilter normalizes them, so a Paper Tags row can be told from one
    // that stands for nothing. Read off the paper nodes rather than the tag
    // NODES because the filter reads the paper's own list -- but through
    // _normTag, which applies graph.rs's own trim-and-drop-empty to that list,
    // so the set names the chips the canvas actually draws.
    _paperTagSet = new Set();
    // And the same index for the Projects / Project Tags lists: the project ids
    // this payload's papers carry, which is the universe filterGraph resolves a
    // Projects row against (`d.project_ids.some(...)`). `/api/graph/
    // project-options` answers with every ACTIVE project, papers on this canvas
    // or not, so the two lists need this to tell a row that filters from one
    // that only empties the canvas.
    _paperProjectIds = new Set();
    nodes.forEach(n => {
        if (n.type !== 'paper') return;
        if (Array.isArray(n.tags)) {
            n.tags.forEach(t => {
                const norm = _normTag(t);
                if (norm) _paperTagSet.add(norm);
            });
        }
        if (Array.isArray(n.project_ids)) {
            n.project_ids.forEach(id => _paperProjectIds.add(id));
        }
    });

    // Index paper → author labels off the same edge list (backend emits
    // paper→author, but tolerate either orientation).
    const authorLabelById = new Map();
    nodes.forEach(n => {
        if (n.type === 'author') authorLabelById.set(String(n.id), String(n.label || '').toLowerCase());
    });
    _paperAuthorLabels = new Map();
    _allEdgeDefs.forEach(e => {
        const authorId = authorLabelById.has(e.target) ? e.target
                       : authorLabelById.has(e.source) ? e.source
                       : null;
        if (authorId === null) return;
        const paperId = authorId === e.target ? e.source : e.target;
        const labels = _paperAuthorLabels.get(paperId);
        if (labels) labels.push(authorLabelById.get(authorId));
        else _paperAuthorLabels.set(paperId, [authorLabelById.get(authorId)]);
    });

    const cyElements = [
        ...nodes.map(n => {
            const sn = _simNodeById.get(String(n.id));
            return {
                group: 'nodes',
                data: {
                    id:          String(n.id),
                    source_id:   n.source_id   || null,
                    author_id:   n.author_id   ?? null,
                    // A tag node is shared by every paper carrying the tag, so
                    // its chip is drawn with the TAG table's spelling rather
                    // than with whichever paper graph.rs reached first (see
                    // _canonicalTagLabel). Paper and author labels are already
                    // single-owner -- PAPER.TITLE and AUTHOR.AUTHOR_FULL_NAME
                    // -- and pass through untouched.
                    label:       n.type === 'tag' ? _canonicalTagLabel(n.label) : n.label,
                    type:        n.type,
                    category:    n.category    || null,
                    tags:        n.tags        || [],
                    has_pdf:     n.has_pdf     || false,
                    published:   _publishedOrNull(n.published),
                    project_ids: n.project_ids || [],
                    url:         n.url         || null,
                    doi:         n.doi         || null,
                    summary:     n.summary     || '',
                },
                position: { x: sn.x, y: sn.y },
            };
        }),
        ...edges.map(e => ({
            group: 'edges',
            data: { source: String(e.source), target: String(e.target) },
        })),
    ];

    cy = cytoscape({
        container: document.getElementById('cy'),
        elements:  cyElements,
        style:     cytoscapeStyle(),
        layout:    { name: 'preset' },
        userZoomingEnabled: true,
        userPanningEnabled: true,
        minZoom: 0.05,
        maxZoom: 10,
        // Cache a texture and drop edges while panning/zooming to keep the
        // viewport smooth as the node count grows.
        textureOnViewport: true,
        hideEdgesOnViewport: true,
    });

    if (preserveView && prevZoom != null && prevPan) {
        cy.viewport({ zoom: prevZoom, pan: prevPan });
    } else {
        _fitGraph();
    }

    // Cache each node's handle so the per-tick sync below skips a getElementById
    // lookup (and its allocation) per node per frame.
    simNodes.forEach(sn => { sn.cyNode = cy.getElementById(sn.id); });

    cy.on('grab', 'node', e => {
        _fitOnSettle = false;  // user took control — don't reframe under them
        _hideNodeTooltip();
        const sn = _simNodeById.get(e.target.id());
        if (sn) { sn.fx = sn.x; sn.fy = sn.y; }
        if (simulation) simulation.alphaTarget(0.3).restart();
    });
    cy.on('drag', 'node', e => {
        const sn  = _simNodeById.get(e.target.id());
        const pos = e.target.position();
        if (sn) { sn.fx = pos.x; sn.fy = pos.y; }
    });
    // Releasing a drag hands the node back to the layout -- unless an active
    // filter had EXCLUDED it, in which case the pin under the user's fingers
    // was the filter's own. An excluded node is an 8% ghost, not a hidden one,
    // and _eventsFor only takes a node out of hit-testing at opacity 0 (i.e.
    // isolate mode), so every ghost on the canvas is fully grabbable. Nulling
    // fx/fy there released a pin filterGraph owns and nothing was going to
    // restore until the next filter pass: the ghost -- charge 0, collision
    // radius 0, so nothing else can hold it up -- slid off towards the origin
    // under the centring force on its own, with no filter control touched to
    // explain it. Re-pin at the drop point instead, and keep _filterPinned set
    // so the pass that re-admits the node still knows the pin is the filter's
    // to release. This is the mirror of filterGraph's own guard, which exists
    // so a drag pin is never cleared out from under the user; the same slot
    // has two writers and only one of them carried the rule.
    cy.on('free', 'node', e => {
        const sn = _simNodeById.get(e.target.id());
        if (sn) {
            if (_layoutNodeIds && !_layoutNodeIds.has(e.target.id())) {
                if (sn.fx == null) { sn.fx = sn.x; sn.fy = sn.y; }
                sn._filterPinned = true;
            } else {
                sn.fx = null; sn.fy = null; sn._filterPinned = false;
            }
        }
        if (simulation) simulation.alphaTarget(0);
    });

    // Click paper node:
    //   Regular click  → clear selection + navigate to the paper
    //   Ctrl/Cmd click → toggle additive (no navigation)
    cy.on('tap', 'node[type = "paper"]', e => {
        const paper_id = e.target.id();
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) {
            _toggleSelection(paper_id);
        } else {
            _navigateAway({ type: 'paper_clicked', id: paper_id });
        }
    });

    // Click author node → open its author page. Skip Ctrl/Cmd (reserved for paper
    // multi-select) and nodes with no resolved AUTHOR_FK.
    cy.on('tap', 'node[type = "author"]', e => {
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) return;
        const authorId = e.target.data('author_id');
        if (authorId === null || authorId === undefined) return;
        _navigateAway({ type: 'author_clicked', id: String(authorId) });
    });

    // Click tag node → open its tag page, the same page TagBadge links to from
    // every other surface in the app. Papers and authors have always navigated;
    // a tag was the one node type on this canvas that did nothing at all when
    // clicked, with no cue that it was inert. Skip Ctrl/Cmd (reserved for paper
    // multi-select) and tags with no label to route on.
    cy.on('tap', 'node[type = "tag"]', e => {
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) return;
        const label = e.target.data('label');
        if (!label) return;
        _navigateAway({ type: 'tag_clicked', label: String(label) });
    });

    // Tap background → clear selection (unless Ctrl/Cmd held)
    cy.on('tap', e => {
        if (e.target === cy && !e.originalEvent.ctrlKey && !e.originalEvent.metaKey) {
            clearSelection();
        }
    });

    // Hover a node → inspector + pointer cursor. cytoscape only dispatches to
    // elements it considers interactive (`events`), and _applyAllStyles sets
    // `events: 'no'` on everything it takes to opacity 0, so a node the isolate
    // filter or a Visibility checkbox removed cannot be hovered either -- the
    // same rule that keeps it from being clicked.
    cy.on('mouseover', 'node', e => _showNodeTooltip(e.target));
    cy.on('mouseout',  'node', () => _hideNodeTooltip());
    // The box is placed in rendered (screen) coordinates, so a pan or zoom
    // would leave it pointing at empty canvas. Cheaper than tracking the node.
    cy.on('viewport', () => _hideNodeTooltip());

    const cs = parseFloat($('centerForce').value);
    simulation = d3.forceSimulation(simNodes)
        .force('link',      d3.forceLink(simLinks).id(d => d.id)
                              .distance(parseFloat($('linkDistance').value))
                              .strength(parseFloat($('linkStrength').value)))
        .force('charge',    _chargeForce())
        .force('x',         d3.forceX(0).strength(cs))
        .force('y',         d3.forceY(0).strength(cs))
        .force('collision', _collideForce());

    simulation.on('tick', () => {
        cy.batch(() => {
            simNodes.forEach(d => d.cyNode.position({ x: d.x, y: d.y }));
        });
    });

    // Frame the settled layout rather than the random seed positions fitted
    // above. Fires again after every drag/filter restart, so the one-shot flag
    // is what keeps it from yanking the viewport out from under the user.
    simulation.on('end', () => {
        if (!_fitOnSettle) return;
        _fitOnSettle = false;
        _fitGraph();
    });

    // Drop selected ids the reload removed (_applyFilter re-renders selection),
    // then re-notify the host so its count stays authoritative.
    _selectedIds.forEach(id => { if (cy.getElementById(id).empty()) _selectedIds.delete(id); });

    // Repaint the Paper Tags rows against THIS payload. The rows are free text
    // the user typed, but what they RESOLVE to is not, and it just changed: a
    // tag deleted or renamed elsewhere in the app leaves a row that now matches
    // no paper, and nothing else on the reload path redraws that list. Same
    // reason setFilterOptions() redraws the project rows.
    _renderTagRows();
    // And the same reason one level up, for the list those rows are added FROM:
    // the offer is narrowed to the tags this payload's papers carry, so a tag
    // that left the graph in this reload stops being suggested rather than
    // waiting for `/api/tags` to answer again (it never will — the TAG row
    // outlives the paper link).
    _renderTagDatalist();
    // The Projects and Project Tags lists take the same two passes for the same
    // reason, and this is the half they were missing: setFilterOptions redraws
    // them when the OPTIONS change, but which projects have a paper on the
    // canvas is a fact about the PAYLOAD, and fetchAndLoadGraph installs the
    // options BEFORE this function builds _paperProjectIds -- so without this
    // both dropdowns and both row lists would spend every load one payload
    // behind. (A Refresh after the last paper left a project is the case: the
    // project is still active, so the options answer is unchanged and nothing
    // else would repaint.)
    _renderProjectDatalists();
    _renderProjectRows();
    _renderProjTagRows();

    _applyFilter();
    _notifySelectionChanged();
}

function getThemeColors() {
    const s = getComputedStyle(document.documentElement);
    return {
        accent: s.getPropertyValue('--color-accent').trim() || '#5b8dee',
        bg:     s.getPropertyValue('--color-bg').trim()     || '#0f0f1a',
        border: s.getPropertyValue('--color-border').trim() || '#2e2e50',
        muted:  s.getPropertyValue('--color-muted').trim()  || '#7777aa',
        text:   s.getPropertyValue('--color-text').trim()   || '#ccccdd',
        success: s.getPropertyValue('--color-success').trim() || '#4caf88',
        danger:  s.getPropertyValue('--color-danger').trim()  || '#e05c6c',
    };
}

// Pull the node-type colours off the current --color-* vars. Cheap enough to
// re-run on every theme change; callers repaint cytoscape themselves.
function _syncNodeColors() {
    const t = getThemeColors();
    PAPER_COLOR     = t.accent;
    TAG_COLOR       = t.success;
    HIGHLIGHT_COLOR = t.danger;
}

function cytoscapeStyle() {
    const t = getThemeColors();
    return [
        {
            selector: 'node[type = "paper"]',
            style: {
                'shape':                'ellipse',
                'width':                20,
                'height':               20,
                'background-color':     t.accent,
                'label':                'data(label)',
                'font-size':            13,
                'font-weight':          600,
                // Theme text with a background-colored halo over edges/nodes.
                'color':                t.text,
                'text-outline-color':   t.bg,
                'text-outline-width':   2.5,
                'text-outline-opacity': 1,
                // Stop rendering labels below ~7px on-screen.
                'min-zoomed-font-size': 7,
                'font-family':          LABEL_FONT,
                'text-valign':          'center',
                'text-halign':          'right',
                'text-margin-x':        8,
                'text-max-width':       '180px',
                'text-wrap':            'ellipsis',
                'border-width':         1.5,
                'border-color':         t.bg,
            },
        },
        {
            selector: 'node[type = "author"]',
            style: {
                'shape':                'diamond',
                'width':                14,
                'height':               14,
                'background-color':     AUTHOR_COLOR,
                'label':                'data(label)',
                'font-size':            12,
                'font-weight':          600,
                'color':                t.text,
                'text-outline-color':   t.bg,
                'text-outline-width':   2.5,
                'text-outline-opacity': 1,
                'min-zoomed-font-size': 7,
                'font-family':          LABEL_FONT,
                'text-valign':          'center',
                'text-halign':          'right',
                'text-margin-x':        7,
                'text-max-width':       '140px',
                'text-wrap':            'ellipsis',
            },
        },
        {
            selector: 'node[type = "tag"]',
            style: {
                'shape':                'roundrectangle',
                'width':                'label',
                'height':               20,
                'padding':              '0 7px',
                'background-color':     TAG_COLOR,
                'label':                'data(label)',
                'font-size':            12,
                'font-weight':          600,
                // Label sits inside the chip: white text over a neutral scrim, so
                // it stays readable whatever hue --color-success resolves to.
                'color':                '#ffffff',
                'text-outline-color':   'rgba(0,0,0,0.55)',
                'text-outline-width':   1.5,
                'text-outline-opacity': 1,
                'min-zoomed-font-size': 7,
                'font-family':          LABEL_FONT,
                'text-valign':          'center',
                'text-halign':          'center',
                'border-width':         0,
            },
        },
        {
            selector: 'edge',
            style: {
                'width':       1.5,
                'line-color':  t.border,
                'curve-style': 'haystack',
            },
        },
    ];
}

// ── Filter ────────────────────────────────────────────────────────────────────

// ── Filtered-to-nothing notice ───────────────────────────────────────────────
// Every other list in the app says so when a filter matches nothing; this
// canvas said nothing at all. With "Show highlighted only" on, a filter that
// matches no paper paints an empty rectangle; with it off, a field of 8%
// ghosts. Both look exactly like a graph that failed to load, and the host's
// loading / empty / error overlay cannot help: `graph_loaded.nodeCount` counts
// what the BACKEND sent, so a filtered-out graph is still "ready", and the
// overlay is `absolute inset-0` — drawing it here would bury the filter panels
// the user has to reach to undo the filter. So the guest owns this one state.

function _positionNoMatchNotice(box) {
    // Measure from the left edge first, for the same reason the hover inspector
    // does: a fixed box with a max-width shrinks to fit the room left of the
    // viewport edge, so measuring it where the last show parked it reads a
    // stale width.
    box.style.left = '0px';
    const rect = box.getBoundingClientRect ? box.getBoundingClientRect() : null;
    const w    = (rect && rect.width) || 0;
    const vw   = window.innerWidth || 0;
    if (!vw) return;  // no layout (standalone/no-DOM): leave it at the edge
    const avail = vw - _panelGutter(vw);
    box.style.left = Math.max(TOOLTIP_MARGIN, (avail - w) / 2) + 'px';
}

// The Filters > Author box matches a paper through _paperAuthorLabels, which
// loadGraph builds from the paper-to-author EDGES. "Hide single-paper
// authors" is applied by the backend (crates/core/src/graph.rs joins
// author_paper_counts in author_rows_sql), and it drops the author's node AND
// its edges, so with the option on an author with exactly one paper is not
// merely undrawn -- they are unmatchable, and typing a name that is certainly
// in the library empties the canvas under "No papers match the active
// filters". True, but not why. The option is a checkbox in the host's page
// header, invisible and unreachable from inside this frame, so the notice is
// the only place that can say so.
const AUTHOR_HIDDEN_HINT =
    'Authors with a single paper are hidden, so the Author filter cannot match them.';

// `drawn` counts the nodes left at full opacity, i.e. what the user can
// actually read. `total` is what the backend sent: an empty library is the
// HOST's EmptyState to draw, so this stays quiet for it rather than stacking a
// second "nothing here" panel on the same rectangle. `authorFilter` is the
// Author box's current text, which decides whether the hidden-authors hint
// above is relevant at all.
function _updateNoMatchNotice(drawn, total, authorFilter) {
    const box = $('no-match-notice');
    if (!box) return;
    const hint    = $('no-match-hint');
    const authBtn = $('no-match-show-authors');
    if (!(total > 0) || drawn > 0) {
        box.style.display = 'none';
        box.setAttribute('aria-hidden', 'true');
        // Reset rather than leave behind: the card is hidden, but the hint is
        // the reason for a state that no longer holds.
        if (hint)    { hint.textContent = ''; hint.style.display = 'none'; }
        if (authBtn) authBtn.style.display = 'none';
        return;
    }
    // Three Visibility checkboxes off is a different mistake from a filter that
    // excludes everything, and "Clear all filters" fixes both (it re-checks the
    // boxes as well), so one notice with two bodies covers it.
    const allTypesHidden = _hiddenTypes.size === 3;
    $('no-match-title').textContent = allTypesHidden ? 'Nothing to draw' : 'No matches';
    $('no-match-body').textContent  = allTypesHidden
        ? 'Papers, Authors and Tags are all switched off under Filters \u203a Visibility.'
        : 'No papers match the active filters.';
    // Nothing here can tell whether a hidden author is the actual cause -- the
    // names never arrived -- so this is offered as a second possibility, and
    // only when both halves of it are in force. Not raised for the
    // all-types-hidden body, which already names its own single cause.
    const authorsMayBeHidden = !allTypesHidden && !!authorFilter && _excludeSingleAuthors;
    if (hint) {
        hint.textContent   = authorsMayBeHidden ? AUTHOR_HIDDEN_HINT : '';
        hint.style.display = authorsMayBeHidden ? '' : 'none';
    }
    if (authBtn) authBtn.style.display = authorsMayBeHidden ? '' : 'none';
    box.style.display = '';
    box.setAttribute('aria-hidden', 'false');
    _positionNoMatchNotice(box);
}

function filterGraph(opts) {
    if (!cy) return;

    const {
        showAuthors  = true,
        showPapers   = true,
        showTags     = true,
        category     = null,
        hasPdf       = false,
        highlight    = null,
        authorFilter = null,
        dateFrom     = null,
        dateTo       = null,
        isolate      = false,
        projectIds   = null,
        projTagIds   = null,
    } = opts;

    const hlLower   = highlight    ? highlight.toLowerCase()    : null;
    const authLower = authorFilter ? authorFilter.toLowerCase() : null;
    // Tag labels are UNIQUE COLLATE NOCASE in the TAG table, so every tag match
    // in the app is case-insensitive (_evalTagFilter folds the paper-tag rows
    // the same way). Folded once here rather than per candidate node.
    const projTagSet = projTagIds
        ? new Set(projTagIds.map(t => String(t).toLowerCase()))
        : null;

    // The three Visibility checkboxes are a RENDER concern — which node TYPES
    // get drawn — and are deliberately kept apart from the attribute filters,
    // which decide which papers MATCH. Folding them together is what made
    // unchecking "Papers" blank the entire canvas: it emptied the paper set,
    // and author/tag visibility is derived purely from edges to matching
    // papers, so authors and tags went with it. Match first, hide after.
    const matchedPaperIds = new Set();
    cy.nodes('[type = "paper"]').forEach(n => {
        const d = n.data();
        if (category && !d.category?.toLowerCase().includes(category.toLowerCase())) return;
        if (hasPdf && !d.has_pdf) return;
        if (!_evalTagFilter(d.tags)) return;
        if (projectIds && !(Array.isArray(d.project_ids) && d.project_ids.some(id => projectIds.includes(id)))) return;
        if (projTagSet) {
            const pids = Array.isArray(d.project_ids) ? d.project_ids : [];
            const hasProjTag = pids.some(pid => {
                const proj = _projectMap.get(pid);
                if (!proj || !Array.isArray(proj.tags)) return false;
                return proj.tags.some(t => projTagSet.has(String(t).toLowerCase()));
            });
            if (!hasProjTag) return;
        }
        if (hlLower && !d.label.toLowerCase().includes(hlLower)) return;
        if (dateFrom && d.published && d.published < dateFrom) return;
        if (dateTo   && d.published && d.published > dateTo)   return;
        if (authLower) {
            const authorLabels = _paperAuthorLabels.get(n.id()) || [];
            if (!authorLabels.some(a => a.includes(authLower))) return;
        }
        matchedPaperIds.add(n.id());
    });

    const matchedAuthorIds = new Set();
    cy.nodes('[type = "author"]').forEach(a => {
        a.connectedEdges().forEach(e => {
            const other = e.source().id() === a.id() ? e.target() : e.source();
            if (matchedPaperIds.has(other.id())) matchedAuthorIds.add(a.id());
        });
    });

    const matchedTagIds = new Set();
    cy.nodes('[type = "tag"]').forEach(t => {
        t.connectedEdges().forEach(e => {
            const other = e.source().id() === t.id() ? e.target() : e.source();
            if (matchedPaperIds.has(other.id())) matchedTagIds.add(t.id());
        });
    });

    _visiblePaperIds  = matchedPaperIds;
    _visibleAuthorIds = matchedAuthorIds;
    _visibleTagIds    = matchedTagIds;
    _filterIsolate    = isolate;
    // A type the user switched off is removed, not dimmed — the attribute
    // filters own the 8% ghost, a Visibility checkbox means "don't draw this".
    _hiddenTypes = new Set();
    if (!showPapers)  _hiddenTypes.add('paper');
    if (!showAuthors) _hiddenTypes.add('author');
    if (!showTags)    _hiddenTypes.add('tag');

    _applyAllStyles();
    // A filter pass changes how much of the selection is DRAWN without touching
    // the selection itself, so nothing else on this path refreshes the counter:
    // switching Papers off, or turning isolate on, silently takes selected
    // papers off the canvas while the header still reads a bare "(5)".
    _updateSelectionCount();

    // A hidden node type contributes nothing readable even though its members
    // still "match", so the notice counts what is drawn, not what matched.
    _updateNoMatchNotice(
        (_hiddenTypes.has('paper')  ? 0 : matchedPaperIds.size)
      + (_hiddenTypes.has('author') ? 0 : matchedAuthorIds.size)
      + (_hiddenTypes.has('tag')    ? 0 : matchedTagIds.size),
        cy.nodes().length,
        authLower
    );

    // Physics: remove nodes the attribute filters excluded from the simulation
    // forces so they don't push/pull the matching ones at all. This tracks the
    // MATCHED sets, not the rendered ones: hiding a node type is a view toggle,
    // so the layout must not reshuffle underneath it. Deriving it from the
    // rendered sets meant unchecking "Papers" dropped every link (every edge
    // has a paper endpoint) and left the authors and tags to fly apart under
    // pure repulsion.
    if (simulation) {
        const layoutNodeIds = new Set([
            ...matchedPaperIds, ...matchedAuthorIds, ...matchedTagIds,
        ]);

        // Pin excluded nodes in place; release the pins the filter owns once a
        // node is back in the layout. Tracked via _filterPinned so a drag pin
        // (fx/fy set by the grab handler) is never cleared out from under it,
        // and so isolate-mode toggles don't permanently freeze the layout.
        _simNodeById.forEach((sn, id) => {
            if (!layoutNodeIds.has(id)) {
                if (sn.fx == null) { sn.fx = sn.x; sn.fy = sn.y; sn._filterPinned = true; }
            } else if (sn._filterPinned) {
                sn.fx = null; sn.fy = null; sn._filterPinned = false;
            }
        });

        // Restrict link force to edges where both endpoints are in the layout
        const activeLinks = _allEdgeDefs
            .filter(e => layoutNodeIds.has(e.source) && layoutNodeIds.has(e.target))
            .map(e => ({ ...e }));
        simulation.force('link').links(activeLinks);

        // Zero out charge AND collision radius for excluded nodes, so they
        // neither repel/attract nor take up room. Published to the module so
        // the Repel force slider rebuilds the charge with the same membership
        // rather than a flat strength.
        _layoutNodeIds = layoutNodeIds;
        simulation.force('charge', _chargeForce());
        simulation.force('collision', _collideForce());

        simulation.alpha(0.3).restart();
    }
}

// ── Unified visual state ──────────────────────────────────────────────────────
// Applies both filter visibility and selection highlight in one pass.

// cytoscape decides what a click can land on from `events` / `visibility` /
// `display` and never from opacity (see eleInteractive in cytoscape.min.js), so
// an element the isolate filter has taken to opacity 0 stays fully hit-testable:
// tapping apparently blank canvas navigated to a paper the user could not see,
// dragged an invisible node, and swallowed the background tap that clears the
// selection. Tie interactivity to visibility instead. Every branch below sets
// `events` explicitly because these are per-element bypasses — an unset one
// keeps whatever the previous filter pass wrote.
const _eventsFor = op => (op === 0 ? 'no' : 'yes');

function _applyAllStyles() {
    if (!cy) return;

    const anySelected   = _selectedIds.size > 0;
    const filterActive  = _visiblePaperIds !== null;
    const filterHideOp  = _filterIsolate ? 0 : DIM_OPACITY;

    // Neighbour ids (authors + tags) connected to any selected paper
    const selAuthorIds = new Set();
    const selTagIds    = new Set();
    if (anySelected) {
        _selectedIds.forEach(pid => {
            const n = cy.getElementById(pid);
            n.connectedEdges().forEach(e => {
                const other = e.source().id() === pid ? e.target() : e.source();
                if (other.data('type') === 'author') selAuthorIds.add(other.id());
                if (other.data('type') === 'tag')    selTagIds.add(other.id());
            });
        });
    }

    // Filtered out → filter dim (0 under isolate); selected → full; visible but
    // unselected while something is selected → soft dim; otherwise full.
    const opacityFor = (filterVisible, selected) =>
        !filterVisible ? filterHideOp
      : selected       ? FULL_OPACITY
      : anySelected    ? SEL_DIM_OPACITY
      :                  FULL_OPACITY;

    const paperHidden  = _hiddenTypes.has('paper');
    const authorHidden = _hiddenTypes.has('author');
    const tagHidden    = _hiddenTypes.has('tag');

    cy.batch(() => {
        cy.nodes('[type = "paper"]').forEach(n => {
            const nid = n.id();
            const filterVisible = !filterActive || (_visiblePaperIds && _visiblePaperIds.has(nid));
            const selected = anySelected && _selectedIds.has(nid);
            const opacity = paperHidden ? 0 : opacityFor(filterVisible, selected);
            n.style({
                'opacity':          opacity,
                'events':           _eventsFor(opacity),
                // The selection is painted on EVERY selected paper, including
                // one an attribute filter has excluded. Such a node is an 8%
                // ghost, not a hidden one, and _eventsFor only drops it out of
                // hit-testing at opacity 0 -- so it is fully clickable and a
                // Ctrl-click puts it straight into the selection. Withholding
                // the highlight there meant that click changed nothing on the
                // canvas at all: the count in the panel header (and the host's
                // "N selected" / "Add to Project" bar, which mirrors it) went
                // up by one with no way to tell WHICH paper had joined, and a
                // selection built before the filter was typed went the same
                // way -- acted on by the host, invisible on the graph. The
                // filter still owns opacity, so the ghost stays a ghost; the
                // selection owns colour, and now says so.
                'background-color': selected ? HIGHLIGHT_COLOR : PAPER_COLOR,
            });
        });

        cy.nodes('[type = "author"]').forEach(n => {
            const nid = n.id();
            const filterVisible = !filterActive || (_visibleAuthorIds && _visibleAuthorIds.has(nid));
            const opacity = authorHidden ? 0 : opacityFor(filterVisible, selAuthorIds.has(nid));
            n.style({ 'opacity': opacity, 'events': _eventsFor(opacity) });
        });

        cy.nodes('[type = "tag"]').forEach(n => {
            const nid = n.id();
            const filterVisible = !filterActive || (_visibleTagIds && _visibleTagIds.has(nid));
            const opacity = tagHidden ? 0 : opacityFor(filterVisible, selTagIds.has(nid));
            n.style({ 'opacity': opacity, 'events': _eventsFor(opacity) });
        });

        cy.edges().forEach(e => {
            const sid = e.source().id(), tid = e.target().id();
            const srcFilterVis = !filterActive
                || (_visiblePaperIds  && _visiblePaperIds.has(sid))
                || (_visibleAuthorIds && _visibleAuthorIds.has(sid))
                || (_visibleTagIds    && _visibleTagIds.has(sid));
            const tgtFilterVis = !filterActive
                || (_visiblePaperIds  && _visiblePaperIds.has(tid))
                || (_visibleAuthorIds && _visibleAuthorIds.has(tid))
                || (_visibleTagIds    && _visibleTagIds.has(tid));
            const srcSel = _selectedIds.has(sid) || selAuthorIds.has(sid) || selTagIds.has(sid);
            const tgtSel = _selectedIds.has(tid) || selAuthorIds.has(tid) || selTagIds.has(tid);
            // An edge is only as visible as its endpoints: hiding either type
            // takes the edge with it, so no line dangles into empty canvas.
            const endpointHidden = _hiddenTypes.has(e.source().data('type'))
                                || _hiddenTypes.has(e.target().data('type'));
            const opacity = endpointHidden
                ? 0
                : opacityFor(srcFilterVis && tgtFilterVis, srcSel || tgtSel);
            e.style({ 'opacity': opacity, 'events': _eventsFor(opacity) });
        });
    });
}

// ── Selection (click to set, Ctrl+click to toggle) ───────────────────────────

function _toggleSelection(paperId) {
    if (_selectedIds.has(paperId)) {
        _selectedIds.delete(paperId);
    } else {
        _selectedIds.add(paperId);
    }
    _applyAllStyles();
    _notifySelectionChanged();
}

// A click that takes the user off the graph. Every such click drops the local
// selection first: the host clears its own copy when it navigates, and AppShell
// keeps this iframe alive across route changes, so a selection left behind here
// comes back highlighted with a "Selection (n)" counter the host disagrees with
// and no action bar to act on it. The host owns its count via the *_clicked
// handler it is about to receive, so the selection_changed post is skipped.
function _navigateAway(msg) {
    // AppShell keeps this iframe alive across the route change, so a box left
    // showing here is what the user comes back to.
    _hideNodeTooltip();
    _selectedIds.clear();
    _applyAllStyles();
    _updateSelectionCount();
    _notifyHost(msg);
}

function _notifySelectionChanged() {
    const sourceIds = [];
    if (cy) {
        _selectedIds.forEach(nid => {
            const sid = cy.getElementById(nid).data('source_id');
            if (sid) sourceIds.push(sid);
        });
    }
    _updateSelectionCount();
    window.parent.postMessage({ type: 'selection_changed', sourceIds }, window.location.origin);
}

// Selected papers the current filter state does not DRAW at all, i.e. the ones
// whose computed opacity in _applyAllStyles is 0: everything when the Papers
// visibility checkbox is off, and the non-matching ones under isolate ("Show
// highlighted only"). Painting the highlight covers the ordinary 8% ghost, but
// these are genuinely off the canvas, so the only honest place left to report
// them is the count itself.
function _hiddenSelectedCount() {
    if (!cy || _selectedIds.size === 0) return 0;
    if (_hiddenTypes.has('paper')) return _selectedIds.size;
    if (!_filterIsolate || _visiblePaperIds === null) return 0;
    let n = 0;
    _selectedIds.forEach(nid => { if (!_visiblePaperIds.has(nid)) n++; });
    return n;
}

function _updateSelectionCount() {
    const counter = $('selectionCount');
    if (!counter) return;
    const hidden = _hiddenSelectedCount();
    counter.textContent = hidden > 0
        ? `(${_selectedIds.size}, ${hidden} hidden)`
        : `(${_selectedIds.size})`;
}

function selectAllPapers() {
    if (!cy) return;
    // Nothing to select when papers aren't being drawn.
    if (_hiddenTypes.has('paper')) return;
    const visible = _visiblePaperIds;
    cy.nodes('[type = "paper"]').forEach(n => {
        if (visible === null || visible.has(n.id())) {
            _selectedIds.add(n.id());
        }
    });
    _applyAllStyles();
    _notifySelectionChanged();
}

function clearSelection() {
    _selectedIds.clear();
    _applyAllStyles();
    _notifySelectionChanged();
}

// Replace the selection with exactly the papers the host names. The host never
// sees a node id -- _notifySelectionChanged posts `source_id` strings, so that
// is the only vocabulary it can answer in -- so the ids are mapped back through
// the paper nodes here.
//
// The one caller is the project picker's partial-failure path
// (addToProjectMutationOptions in src/lib/paperMutations.ts): when some of the
// papers could not be added it re-selects exactly the failures, so a retry
// cannot re-add the ones that already made it in. That contract was written for
// the Library page, which OWNS its selection; on the graph the selection lives
// in here and the host only mirrors it, so a narrowing that never crossed the
// frame left this document still holding -- and still drawing -- the whole
// original set, and the next click in here posted it straight back over the
// host's copy, re-adding every paper the retry was supposed to skip.
function setSelectionBySourceIds(sourceIds) {
    const wanted = new Set(sourceIds.map(String));
    _selectedIds.clear();
    if (cy) {
        cy.nodes('[type = "paper"]').forEach(n => {
            const sid = n.data('source_id');
            if (sid != null && wanted.has(String(sid))) _selectedIds.add(n.id());
        });
    }
    _applyAllStyles();
    // Post back rather than letting the host assume the ask was met: an id with
    // no node in the current graph (a paper deleted since the load, or one the
    // backend dropped from the payload) cannot be selected here, and the two
    // copies of the count have to keep meaning the same thing -- the invariant
    // every other path through this function's neighbours already holds.
    _notifySelectionChanged();
}

window.addEventListener('resize', () => {
    if (!cy) return;
    cy.resize();
    // Centred in the canvas gutter, so a width change moves it — including the
    // 0x0 → real-size jump the reveal fires, where it was placed against an
    // unknown viewport and left at the left edge.
    const notice = $('no-match-notice');
    if (notice && notice.style.display !== 'none') _positionNoMatchNotice(notice);
    // Revealing the iframe takes the viewport from 0x0 to its real size and
    // fires this; run the fit that was skipped while there was nothing to fit.
    if (_fitDeferred) _fitGraph();
});

_syncNodeColors();

// ── App keyboard shortcuts ───────────────────────────────────────────────────
// Key events do not cross a frame boundary, so the single window listener in
// src/lib/shortcuts.ts (useGlobalShortcuts, bound on the HOST window by
// AppShell) stops seeing anything the moment focus lands in here -- which is
// the first click on the canvas, since panning, selecting and dragging all
// need one. Every shortcut Settings lists as app-wide therefore died on
// /graph alone, and the webview's own zoom, which that host listener
// suppresses with preventDefault, came back in its place: Ctrl+- over the
// graph zoomed the WEBVIEW instead of the interface -- the two mechanisms
// src/lib/zoom.ts says must never both be live.
//
// The host pushes the combos it currently answers to (rebinds included) and we
// hand the matching keydowns back to it, swallowing the default so only one
// zoom happens. Nothing outside that list is touched, which is what keeps
// Ctrl+A / Ctrl+C working in the filter boxes.
let _shortcutCombos = [];

// Mirrors captureOverride() in src/lib/shortcuts.ts: Ctrl and Cmd are one
// modifier and the key is compared case-insensitively. A combo whose `shift`
// is null matches either -- '+' is Shift+'=' on most layouts, so the built-in
// zoom matchers ignore Shift and these combos have to as well.
function _comboMatches(c, e) {
    return !!c
        && c.ctrl === !!(e.ctrlKey || e.metaKey)
        && c.alt === !!e.altKey
        && (c.shift === null || c.shift === undefined || c.shift === !!e.shiftKey)
        && typeof c.key === 'string' && typeof e.key === 'string'
        && c.key.toLowerCase() === e.key.toLowerCase();
}

window.addEventListener('keydown', function(e) {
    if (!_shortcutCombos.some(c => _comboMatches(c, e))) return;
    // The host's own listener preventDefaults for the same reason; auto-repeat
    // is forwarded too, so holding Ctrl+- keeps zooming as it does everywhere
    // else in the app.
    if (e.preventDefault) e.preventDefault();
    _notifyHost({
        type: 'shortcut_key',
        combo: {
            ctrl:  !!(e.ctrlKey || e.metaKey),
            alt:   !!e.altKey,
            shift: !!e.shiftKey,
            key:   e.key,
        },
    });
});


window.addEventListener('message', function(e) {
    if (!e.data || e.origin !== window.location.origin) return;
    if (e.data.type === 'theme_update') {
        const c = e.data.colors;
        const r = document.documentElement;
        // getColors() resolves all eight tokens; forward every one of them so
        // graph.css and _syncNodeColors() see the same palette the host does.
        THEME_VARS.forEach(function(k) {
            if (typeof c[k] === 'string' && c[k]) r.style.setProperty('--color-' + k, c[k]);
        });
        // Light/dark is not derivable from the eight colour tokens, so the host
        // sends the mode alongside them; without it a dark→light switch leaves
        // every native control in the panel on the old scheme.
        if (COLOR_SCHEMES.indexOf(e.data.mode) !== -1) {
            r.style.setProperty('color-scheme', e.data.mode);
        }
        _syncNodeColors();
        if (cy) {
            cy.style(cytoscapeStyle()).update();
            // The new stylesheet alone does NOT repaint the papers. Every paper
            // node carries a per-element `background-color` set by
            // _applyAllStyles (it is how selection/filter state is painted), and
            // a cytoscape bypass outranks the stylesheet: installing a sheet
            // runs Style.clear() -> cleanElements(eles, keepBypasses = true),
            // which files the new sheet value under the bypass rather than over
            // it. Without this repaint the papers stay on the OLD accent while
            // tags, edges and labels follow the new theme.
            _applyAllStyles();
        }
    } else if (e.data.type === 'clear_selection') {
        clearSelection();
    } else if (e.data.type === 'set_selection') {
        // Host-driven narrowing of the selection; see setSelectionBySourceIds.
        if (Array.isArray(e.data.sourceIds)) setSelectionBySourceIds(e.data.sourceIds);
    } else if (e.data.type === 'set_options') {
        // In-place option change: re-fetch, keeping filters / layout / view.
        if (typeof e.data.excludeSingleAuthors === 'boolean') {
            _excludeSingleAuthors = e.data.excludeSingleAuthors;
        }
        fetchAndLoadGraph({ preserveView: true });
    } else if (e.data.type === 'refresh') {
        fetchAndLoadGraph({ preserveView: true });
    } else if (e.data.type === 'set_shortcuts') {
        // Resent whenever the user rebinds one in Settings, so the list the
        // keydown listener above filters on never goes stale.
        _shortcutCombos = Array.isArray(e.data.combos) ? e.data.combos : [];
    }
});

// ── Data fetch + bootstrap ───────────────────────────────────────────────────
// The iframe can't use Tauri invoke, so it fetches the in-process backend over
// the linxiv:// scheme (see src-tauri protocol.rs, which bridges exactly the
// four GET endpoints read below).
//
// WHICH backend that is cannot be sniffed from inside the guest. `tauri dev`
// and browser dev both serve this document from http://localhost:5180, but the
// app around it talks to two entirely different backends in those two cases:
// under `tauri dev` src/api/client.ts goes through invoke() to the IN-PROCESS
// library, while in a plain browser it fetches /api and Vite proxies it to a
// separate dev server on :8000. Keying off the URL therefore sent the graph —
// and only the graph — to the dev server under `tauri dev`: a different
// database, usually not even running, so the canvas showed someone else's
// library or an error while every other surface showed the real one. The rest
// of the app already keys this decision off `isTauri` instead (papers.ts's
// getPaperPdfUrl / getPdfProxyUrl), which is knowledge only the host has, so
// the host names the transport in the src (src/lib/graphIframeSrc.ts):
//   'linxiv' → the in-process backend over the custom scheme;
//   'origin' → this document's own origin, i.e. browser dev behind the Vite
//              proxy.
// Absent (opened standalone, with no host to ask) falls back to sniffing.
// file: = Qt bridge (no fetch — skip).
const API_TRANSPORTS = ['linxiv', 'origin'];

// The custom scheme is served as http://linxiv.localhost on Windows and
// linxiv://localhost everywhere else (Tauri docs) — the same split, on the same
// userAgent test, as src/api/papers.ts's linxivUrl(). Pinned equal to it in
// graphIframeAssets.test.ts.
function _linxivBase() {
    return /Windows/i.test(navigator.userAgent)
        ? 'http://linxiv.localhost' : 'linxiv://localhost';
}

function _resolveGraphBase(transport) {
    if (transport === 'linxiv') return _linxivBase();
    if (transport === 'origin') return window.location.origin;
    // No host to ask. The packaged window can be served as either
    // tauri://localhost OR http://tauri.localhost depending on platform, so
    // don't key off the protocol — only a real dev server
    // (http://localhost:PORT, which proxies /api itself) uses its own origin.
    const { protocol, hostname, origin } = window.location;
    if ((protocol === 'http:' || protocol === 'https:')
        && (hostname === 'localhost' || hostname === '127.0.0.1')) {
        return origin;  // dev server
    }
    if (protocol === 'file:') return null;
    return _linxivBase();
}

// Fetch graph data + dropdowns and (re)load the graph. The token guards against
// overlapping calls: only the most recent fetch applies its results.
//
// Only `/api/graph` is REQUIRED. The other three payloads populate the filter
// datalists and nothing else, so one of them answering 500 must not blank a
// canvas the graph request was perfectly able to draw -- which is what treating
// all four as one all-or-nothing payload did: a transient failure on
// `/api/tags` put the host on "Couldn't load the graph" with the real data
// already in hand. They resolve to null instead and setFilterOptions leaves
// that one list on its last known contents.
//
// The `graph_loaded` reply carries `nodeCount` on success and `error` on
// failure because this canvas has no state of its own to show: an empty
// library, a dead backend and a still-running fetch all paint the same blank
// rectangle. The host (GraphPage) owns the loading / empty / error surface so
// it can use the app's own Spinner and EmptyState, and it needs both fields to
// tell the three apart.
async function fetchAndLoadGraph(opts = {}) {
    if (!_graphBase) {
        _notifyHost({ type: 'graph_loaded', ok: false, error: 'No backend to fetch from',
                      hasGraph: !!cy });
        return;
    }
    const token = ++_loadToken;
    const graphUrl = _graphBase + '/api/graph'
        + (_excludeSingleAuthors ? '?exclude_single_authors=true' : '');
    try {
        const [graphData, catData, tagData, projData] = await Promise.all([
            _fetchJson(graphUrl),
            _fetchJsonOptional(_graphBase + '/api/categories'),
            _fetchJsonOptional(_graphBase + '/api/tags'),
            _fetchJsonOptional(_graphBase + '/api/graph/project-options'),
            // Cheap after the first load (the face is cached), and it runs
            // alongside the four requests, so it costs no extra wall clock.
            _whenLabelFontReady(),
        ]);
        if (token !== _loadToken) return;  // superseded by a newer request
        // Validate the payload before loadGraph destroys the current graph.
        if (!graphData || !Array.isArray(graphData.nodes) || !Array.isArray(graphData.edges)) {
            throw new Error('Malformed graph payload');
        }
        // Installed BEFORE loadGraph, which ends in a filter pass of its own:
        // the Projects and Project Tags rows are free text resolved through
        // _projectMap, so applying the new options afterwards filtered the
        // fresh graph against the PREVIOUS load's project names and tags -- a
        // project renamed or retagged elsewhere kept filtering by its old
        // spelling until the user happened to touch a filter control.
        setFilterOptions(
            _listOrNull(catData, 'categories'),
            // `/api/tags` returns `{tags: [{label, paper_count}]}`; the datalist wants labels.
            _listOrNull(tagData, 'tags', ts => ts.map(t => t && t.label).filter(Boolean)),
            _listOrNull(projData, 'projects', ps => ps.map(p => ({
                id: p.id,
                name: p.name,
                color: p.color,
                tags: p.tags || [],
            })))
        );
        loadGraph(graphData, opts);
        _notifyHost({ type: 'graph_loaded', ok: true, nodeCount: graphData.nodes.length });
    } catch (err) {
        if (token !== _loadToken) return;
        console.error('Graph load failed', err);
        // `hasGraph` is the half of a failure the host cannot work out for
        // itself, and the whole reason a failed REFRESH need not look like a
        // failed load: everything above -- the four requests and the payload
        // validation -- runs BEFORE loadGraph destroys anything, so a backend
        // that answers 500 leaves the settled canvas exactly where it was and
        // the host has no reason to cover it with "Couldn't load the graph".
        // Not something it can assume, though: loadGraph destroys the old
        // cytoscape instance before it builds the new one, so a throw from
        // inside it lands here with the canvas genuinely blank. Report which
        // one this is instead of leaving the host to guess from the screen it
        // last painted.
        _notifyHost({ type: 'graph_loaded', ok: false, error: String(err && err.message || err),
                      hasGraph: !!cy });
    }
}

// One dropdown payload, mapped to the list setFilterOptions wants, or null when
// the request failed or came back in a shape this build does not recognise.
// Null is "leave that datalist alone", never "the library has none of these".
function _listOrNull(payload, key, map) {
    const list = payload && payload[key];
    if (!Array.isArray(list)) return null;
    return map ? map(list) : list;
}

// Cytoscape measures every node label on an offscreen canvas with ctx.font and
// caches the result on the RENDERER, keyed by the label text plus the font
// *style* properties -- not by whether the family had actually arrived. Inter
// is a self-hosted webfont, so on a cold load the labels can all be measured in
// the fallback face and keep those widths for the rest of the session: tag
// chips (`width: 'label'`) come out the wrong size and `text-max-width`
// ellipsizes at the wrong point. Reinstalling the stylesheet later does NOT
// help -- labelDimCache lives on the renderer and its key is unchanged by a
// font that merely finished loading -- so the fix is to have the face in hand
// before the first render.
function _whenLabelFontReady() {
    const fonts = typeof document !== 'undefined' ? document.fonts : null;
    if (!fonts || typeof fonts.load !== 'function') return Promise.resolve();
    // Never let a stalled font request hold the canvas hostage: past the
    // timeout the graph draws in the fallback face rather than not at all.
    return new Promise(resolve => {
        const timer = setTimeout(resolve, FONT_LOAD_TIMEOUT_MS);
        const done = () => { clearTimeout(timer); resolve(); };
        fonts.load('600 13px ' + LABEL_FONT_FAMILY).then(done, done);
    });
}

async function _fetchJson(url) {
    const r = await fetch(url);
    if (!r.ok) throw new Error('HTTP ' + r.status + ' for ' + url);
    return r.json();
}

// _fetchJson for an auxiliary payload: resolves to null instead of rejecting,
// so a dropdown endpoint being down cannot fail the load the graph request
// already succeeded at. Still logged -- a datalist that silently stopped
// updating is worth seeing in the console.
function _fetchJsonOptional(url) {
    return _fetchJson(url).catch(err => {
        console.warn('Graph filter options unavailable', url, err);
        return null;
    });
}

function _notifyHost(msg) {
    window.parent.postMessage(msg, window.location.origin);
}

(function bootstrapWebGraph() {
    const params = new URLSearchParams(window.location.search);
    const transport = params.get('api');
    _graphBase = _resolveGraphBase(
        API_TRANSPORTS.indexOf(transport) !== -1 ? transport : null);
    _excludeSingleAuthors = params.get('excludeSingleAuthors') === '1';
    // No early return when there is no base to fetch from: fetchAndLoadGraph
    // answers the host with `ok: false` in that case, so the app reaches its
    // own error state at once instead of after GraphPage's 8s
    // dropped-reply fallback, staring at a spinner in the meantime.
    fetchAndLoadGraph({ preserveView: false });
})();
