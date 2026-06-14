let PAPER_COLOR       = '#5b8dee';
const AUTHOR_COLOR    = '#e8a838';
const TAG_COLOR       = '#4caf7d';
const HIGHLIGHT_COLOR = '#ff6b6b';
const DIM_OPACITY     = 0.08;   // filter dim (isolate / non-matching)
const SEL_DIM_OPACITY = 0.28;   // softer dim for non-selected nodes
const FULL_OPACITY    = 1.0;

let cy           = null;
let simulation   = null;
let _simNodeById  = new Map();
let _allEdgeDefs  = [];   // [{source: id, target: id}] — original, never mutated by D3
let _debounce    = null;
let _selectedIds  = new Set();

// Filter state (needed so selection style can layer on top)
let _visiblePaperIds  = null;   // null = no filter active
let _visibleAuthorIds = null;
let _visibleTagIds    = null;
let _filterIsolate    = false;

// Tag logic builder state: [{op: 'AND'|'OR', tag: string}]
let _tagRows = [];

let _projectMap = new Map();  // id → {name, color, tags[]}

// paperId → [author label lowercased]; built once per load so the author filter
// doesn't walk connectedEdges() per paper on every keystroke.
let _paperAuthorLabels = new Map();

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
        div.className = 'tag-filter-row';

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
    if (_tagRows.some(r => r.tag === tag)) { input.value = ''; return; }
    _tagRows.push({ op: 'AND', tag });
    input.value = '';
    _renderTagRows();
    _applyFilter();
}

function _evalTagFilter(paperTags) {
    if (_tagRows.length === 0) return true;
    const tags = Array.isArray(paperTags) ? paperTags : [];
    let result = tags.includes(_tagRows[0].tag);
    for (let i = 1; i < _tagRows.length; i++) {
        const has = tags.includes(_tagRows[i].tag);
        result = _tagRows[i].op === 'AND' ? (result && has) : (result || has);
    }
    return result;
}

$('addTagBtn').addEventListener('click', _addTag);
$('tagFilterInput').addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault(); _addTag(); }
});

$('addProjectBtn').addEventListener('click', () =>
    _addToFilterList('filterProject', _projectFilterNames, 'project-filter-rows', 'project-filter-empty'));
$('filterProject').addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault();
        _addToFilterList('filterProject', _projectFilterNames, 'project-filter-rows', 'project-filter-empty'); }
});

$('addProjTagBtn').addEventListener('click', () =>
    _addToFilterList('filterProjectTag', _projTagFilterNames, 'proj-tag-filter-rows', 'proj-tag-filter-empty'));
$('filterProjectTag').addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault();
        _addToFilterList('filterProjectTag', _projTagFilterNames, 'proj-tag-filter-rows', 'proj-tag-filter-empty'); }
});

// ── Layout sliders ───────────────────────────────────────────────────────────

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
bindSlider('repelForce', 'repelForceVal', v => {
    simulation.force('charge', d3.forceManyBody().strength(-v));
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

$('relayout-btn').addEventListener('click', () => {
    if (!simulation) return;
    _simNodeById.forEach(n => {
        n.x = (Math.random() - 0.5) * 800;
        n.y = (Math.random() - 0.5) * 800;
        n.vx = 0; n.vy = 0; n.fx = null; n.fy = null;
    });
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

// ── Selection panel buttons ─────────────────────────────────────────────────

$('select-all-btn').addEventListener('click', () => selectAllPapers());
$('clear-selection-btn').addEventListener('click', () => clearSelection());

function _renderFilterList(rows, containerId, emptyId) {
    const container = $(containerId);
    container.innerHTML = '';
    $(emptyId).style.display = rows.length === 0 ? '' : 'none';
    rows.forEach((name, i) => {
        const div = document.createElement('div');
        div.className = 'tag-filter-row';
        const sp = document.createElement('span');
        sp.style.cssText = 'min-width:34px; flex-shrink:0;';
        div.appendChild(sp);
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
            _renderFilterList(rows, containerId, emptyId);
            _applyFilter();
        });
        div.appendChild(rm);
        container.appendChild(div);
    });
}

function _addToFilterList(inputId, rows, containerId, emptyId) {
    const input = $(inputId);
    const val = input.value.trim();
    if (!val || rows.includes(val)) { input.value = ''; return; }
    rows.push(val);
    input.value = '';
    _renderFilterList(rows, containerId, emptyId);
    _applyFilter();
}

function _projectIdsFromInput() {
    if (_projectFilterNames.length === 0) return null;
    const ids = [];
    _projectFilterNames.forEach(name => {
        const lower = name.toLowerCase();
        [..._projectMap.values()]
            .filter(p => p.name.toLowerCase().includes(lower))
            .forEach(p => ids.push(p.id));
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

function _applyFilter() {
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

// ── Called from Python to populate filter datalists ──────────────────────────

function _escAttr(s) {
    return String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/'/g, '&#39;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function setFilterOptions(categories, tags, projects) {
    $('categoryList').innerHTML = categories.map(c => `<option value="${_escAttr(c)}">`).join('');
    $('tagList').innerHTML = tags.map(t => `<option value="${_escAttr(t)}">`).join('');

    _projectMap.clear();
    const allProjTags = new Set();
    (projects || []).forEach(proj => {
        _projectMap.set(proj.id, proj);
        (proj.tags || []).forEach(t => allProjTags.add(t));
    });
    $('projectList').innerHTML = [..._projectMap.values()]
        .map(p => `<option value="${_escAttr(p.name)}">`)
        .join('');
    $('projectTagList').innerHTML = [...allProjTags].sort()
        .map(t => `<option value="${_escAttr(t)}">`)
        .join('');
}

// ── Reset every filter panel to its default and re-apply ─────────────────────

function clearFilters() {
    _textFilterIds.forEach(id => { $(id).value = ''; });
    $('filterProject').value = '';
    $('filterProjectTag').value = '';
    _checkFilterIds.forEach(id => { $(id).checked = id !== 'filterHasPdf'; });
    $('isolate-btn').classList.remove('active');
    _tagRows.length = 0;
    _renderTagRows();
    _projectFilterNames.length = 0;
    _renderFilterList(_projectFilterNames, 'project-filter-rows', 'project-filter-empty');
    _projTagFilterNames.length = 0;
    _renderFilterList(_projTagFilterNames, 'proj-tag-filter-rows', 'proj-tag-filter-empty');
    _applyFilter();
}

// ── Graph loading ─────────────────────────────────────────────────────────────

function loadGraph(data, opts = {}) {
    const { nodes, edges } = data;
    const { preserveView = false } = opts;

    // Frame the graph once the layout first settles on a fresh load; cleared as
    // soon as the user interacts so we never reframe under an in-progress drag.
    let fitOnSettle = !preserveView;

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
    _simNodeById = new Map();
    _visiblePaperIds  = null;
    _visibleAuthorIds = null;
    _visibleTagIds    = null;

    const simNodes = nodes.map(n => {
        const prev = prevPositions.get(String(n.id));
        return {
            id: String(n.id),
            x:  prev ? prev.x : (Math.random() - 0.5) * 800,
            y:  prev ? prev.y : (Math.random() - 0.5) * 800,
        };
    });
    // Store original edge defs before D3 mutates source/target to object refs
    _allEdgeDefs = edges.map(e => ({ source: String(e.source), target: String(e.target) }));
    const simLinks = _allEdgeDefs.map(e => ({ ...e }));
    simNodes.forEach(n => _simNodeById.set(n.id, n));

    const cyElements = [
        ...nodes.map(n => {
            const sn = _simNodeById.get(String(n.id));
            return {
                group: 'nodes',
                data: {
                    id:          String(n.id),
                    source_id:   n.source_id   || null,
                    author_id:   n.author_id   ?? null,
                    label:       n.label,
                    type:        n.type,
                    category:    n.category    || null,
                    tags:        n.tags        || [],
                    has_pdf:     n.has_pdf     || false,
                    published:   n.published   || null,
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
        cy.fit(undefined, 40);
    }

    // Cache each node's handle so the per-tick sync below skips a getElementById
    // lookup (and its allocation) per node per frame.
    simNodes.forEach(sn => { sn.cyNode = cy.getElementById(sn.id); });

    // Build paper → author-label index in one edge pass so the author filter
    // reads from a Map instead of walking each paper's edges per keystroke.
    _paperAuthorLabels = new Map();
    cy.edges().forEach(e => {
        const src = e.source(), tgt = e.target();
        let paper, author;
        if (src.data('type') === 'paper' && tgt.data('type') === 'author') { paper = src; author = tgt; }
        else if (tgt.data('type') === 'paper' && src.data('type') === 'author') { paper = tgt; author = src; }
        else return;
        const labels = _paperAuthorLabels.get(paper.id());
        const lower = String(author.data('label') || '').toLowerCase();
        if (labels) labels.push(lower);
        else _paperAuthorLabels.set(paper.id(), [lower]);
    });

    cy.on('grab', 'node', e => {
        fitOnSettle = false;  // user took control — don't reframe under them
        const sn = _simNodeById.get(e.target.id());
        if (sn) { sn.fx = sn.x; sn.fy = sn.y; }
        if (simulation) simulation.alphaTarget(0.3).restart();
    });
    cy.on('drag', 'node', e => {
        const sn  = _simNodeById.get(e.target.id());
        const pos = e.target.position();
        if (sn) { sn.fx = pos.x; sn.fy = pos.y; }
    });
    cy.on('free', 'node', e => {
        const sn = _simNodeById.get(e.target.id());
        if (sn) { sn.fx = null; sn.fy = null; sn._filterPinned = false; }
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
            // Clear the local selection + counter so returning to the graph
            // shows no stale highlight. The host owns its own count via the
            // paper_clicked handler, so we skip the selection_changed post.
            _selectedIds.clear();
            _applyAllStyles();
            _updateSelectionCount();
            window.parent.postMessage({ type: 'paper_clicked', id: paper_id }, window.location.origin);
        }
    });

    // Click author node → open its author page. Skip Ctrl/Cmd (reserved for paper
    // multi-select) and nodes with no resolved AUTHOR_FK.
    cy.on('tap', 'node[type = "author"]', e => {
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) return;
        const authorId = e.target.data('author_id');
        if (authorId === null) return;
        window.parent.postMessage({ type: 'author_clicked', id: String(authorId) }, window.location.origin);
    });

    // Tap background → clear selection (unless Ctrl/Cmd held)
    cy.on('tap', e => {
        if (e.target === cy && !e.originalEvent.ctrlKey && !e.originalEvent.metaKey) {
            clearSelection();
        }
    });

    const cs = parseFloat($('centerForce').value);
    simulation = d3.forceSimulation(simNodes)
        .force('link',      d3.forceLink(simLinks).id(d => d.id)
                              .distance(parseFloat($('linkDistance').value))
                              .strength(parseFloat($('linkStrength').value)))
        .force('charge',    d3.forceManyBody().strength(-parseFloat($('repelForce').value)))
        .force('x',         d3.forceX(0).strength(cs))
        .force('y',         d3.forceY(0).strength(cs))
        .force('collision', d3.forceCollide(14));

    simulation.on('tick', () => {
        cy.batch(() => {
            simNodes.forEach(d => d.cyNode.position({ x: d.x, y: d.y }));
        });
    });

    // Frame the graph once the layout settles, rather than the random seed
    // positions fitted above. fitOnSettle is cleared on the first grab so a
    // drag's own settle can't re-fit and jump the viewport.
    simulation.on('end', () => {
        if (!fitOnSettle) return;
        fitOnSettle = false;
        cy.fit(undefined, 40);
    });

    // Drop selected ids the reload removed (_applyFilter re-renders selection),
    // then re-notify the host so its count stays authoritative.
    _selectedIds.forEach(id => { if (cy.getElementById(id).empty()) _selectedIds.delete(id); });

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
    };
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
                'font-family':          'Segoe UI, sans-serif',
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
                'font-family':          'Segoe UI, sans-serif',
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
                // Label sits inside the chip: white text + dark halo for contrast.
                'color':                '#ffffff',
                'text-outline-color':   '#16321f',
                'text-outline-width':   1.5,
                'text-outline-opacity': 1,
                'min-zoomed-font-size': 7,
                'font-family':          'Segoe UI, sans-serif',
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

    const visiblePaperIds = new Set();
    cy.nodes('[type = "paper"]').forEach(n => {
        if (!showPapers) return;
        const d = n.data();
        if (category && !d.category?.toLowerCase().includes(category.toLowerCase())) return;
        if (hasPdf && !d.has_pdf) return;
        if (!_evalTagFilter(d.tags)) return;
        if (projectIds && !(Array.isArray(d.project_ids) && d.project_ids.some(id => projectIds.includes(id)))) return;
        if (projTagIds) {
            const pids = Array.isArray(d.project_ids) ? d.project_ids : [];
            const hasProjTag = pids.some(pid => {
                const proj = _projectMap.get(pid);
                return proj && Array.isArray(proj.tags) && proj.tags.some(t => projTagIds.includes(t));
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
        visiblePaperIds.add(n.id());
    });

    const visibleAuthorIds = new Set();
    if (showAuthors) {
        cy.nodes('[type = "author"]').forEach(a => {
            a.connectedEdges().forEach(e => {
                const other = e.source().id() === a.id() ? e.target() : e.source();
                if (visiblePaperIds.has(other.id())) visibleAuthorIds.add(a.id());
            });
        });
    }

    const visibleTagIds = new Set();
    if (showTags) {
        cy.nodes('[type = "tag"]').forEach(t => {
            t.connectedEdges().forEach(e => {
                const other = e.source().id() === t.id() ? e.target() : e.source();
                if (visiblePaperIds.has(other.id())) visibleTagIds.add(t.id());
            });
        });
    }

    _visiblePaperIds  = visiblePaperIds;
    _visibleAuthorIds = visibleAuthorIds;
    _visibleTagIds    = visibleTagIds;
    _filterIsolate    = isolate;

    _applyAllStyles();

    // Physics: remove non-visible nodes from simulation forces so they
    // don't push/pull visible nodes at all.
    if (simulation) {
        const visibleNodeIds = new Set([
            ...visiblePaperIds, ...visibleAuthorIds, ...visibleTagIds,
        ]);

        // Pin non-visible nodes in place; release the pins the filter owns once
        // a node is visible again. Tracked via _filterPinned so a drag pin
        // (fx/fy set by the grab handler) is never cleared out from under it,
        // and so isolate-mode toggles don't permanently freeze the layout.
        _simNodeById.forEach((sn, id) => {
            if (!visibleNodeIds.has(id)) {
                if (sn.fx == null) { sn.fx = sn.x; sn.fy = sn.y; sn._filterPinned = true; }
            } else if (sn._filterPinned) {
                sn.fx = null; sn.fy = null; sn._filterPinned = false;
            }
        });

        // Restrict link force to edges where both endpoints are visible
        const activeLinks = _allEdgeDefs
            .filter(e => visibleNodeIds.has(e.source) && visibleNodeIds.has(e.target))
            .map(e => ({ ...e }));
        simulation.force('link').links(activeLinks);

        // Zero out charge for non-visible nodes so they don't repel/attract
        const repel = parseFloat($('repelForce').value);
        simulation.force('charge',
            d3.forceManyBody().strength(n => visibleNodeIds.has(n.id) ? -repel : 0)
        );

        simulation.alpha(0.3).restart();
    }
}

// ── Unified visual state ──────────────────────────────────────────────────────
// Applies both filter visibility and selection highlight in one pass.

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

    cy.batch(() => {
        cy.nodes('[type = "paper"]').forEach(n => {
            const nid = n.id();
            const filterVisible = !filterActive || (_visiblePaperIds && _visiblePaperIds.has(nid));

            if (!filterVisible) {
                // Filtered out — show at filter dim (or hidden if isolate)
                n.style({ 'opacity': filterHideOp, 'background-color': PAPER_COLOR });
            } else if (anySelected && _selectedIds.has(nid)) {
                // Selected → highlight color, full opacity
                n.style({ 'opacity': FULL_OPACITY, 'background-color': HIGHLIGHT_COLOR });
            } else if (anySelected) {
                // Visible but not selected → soft dim
                n.style({ 'opacity': SEL_DIM_OPACITY, 'background-color': PAPER_COLOR });
            } else {
                // No selection, filter visible → full
                n.style({ 'opacity': FULL_OPACITY, 'background-color': PAPER_COLOR });
            }
        });

        cy.nodes('[type = "author"]').forEach(n => {
            const nid = n.id();
            const filterVisible = !filterActive || (_visibleAuthorIds && _visibleAuthorIds.has(nid));

            if (!filterVisible) {
                n.style({ 'opacity': filterHideOp });
            } else if (anySelected && selAuthorIds.has(nid)) {
                n.style({ 'opacity': FULL_OPACITY });
            } else if (anySelected) {
                n.style({ 'opacity': SEL_DIM_OPACITY });
            } else {
                n.style({ 'opacity': FULL_OPACITY });
            }
        });

        cy.nodes('[type = "tag"]').forEach(n => {
            const nid = n.id();
            const filterVisible = !filterActive || (_visibleTagIds && _visibleTagIds.has(nid));

            if (!filterVisible) {
                n.style({ 'opacity': filterHideOp });
            } else if (anySelected && selTagIds.has(nid)) {
                n.style({ 'opacity': FULL_OPACITY });
            } else if (anySelected) {
                n.style({ 'opacity': SEL_DIM_OPACITY });
            } else {
                n.style({ 'opacity': FULL_OPACITY });
            }
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

            if (!srcFilterVis || !tgtFilterVis) {
                e.style({ 'opacity': filterHideOp });
            } else if (anySelected) {
                const srcSel = _selectedIds.has(sid) || selAuthorIds.has(sid) || selTagIds.has(sid);
                const tgtSel = _selectedIds.has(tid) || selAuthorIds.has(tid) || selTagIds.has(tid);
                e.style({ 'opacity': (srcSel || tgtSel) ? FULL_OPACITY : SEL_DIM_OPACITY });
            } else {
                e.style({ 'opacity': FULL_OPACITY });
            }
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

function _updateSelectionCount() {
    const counter = $('selectionCount');
    if (counter) counter.textContent = `(${_selectedIds.size})`;
}

function selectAllPapers() {
    if (!cy) return;
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

function getSelectedPaperData() {
    if (!cy) return JSON.stringify({ papers: [], edges: [] });
    const papers = [];
    const edgeSet = [];
    cy.nodes('[type = "paper"]').forEach(n => {
        if (!_selectedIds.has(n.id())) return;
        const d = n.data();
        const authors = [];
        n.connectedEdges().forEach(e => {
            const other = e.source().id() === n.id() ? e.target() : e.source();
            if (other.data('type') === 'author') authors.push(other.data('label'));
        });
        papers.push({
            paper_id:  d.id,
            title:     d.label,
            category:  d.category || '',
            tags:      d.tags || [],
            has_pdf:   d.has_pdf || false,
            published: d.published || '',
            authors:   authors,
            url:       d.url || null,
            doi:       d.doi || null,
            summary:   d.summary || '',
        });
    });
    cy.edges().forEach(e => {
        const sid = e.source().id(), tid = e.target().id();
        if (_selectedIds.has(sid) && _selectedIds.has(tid)) {
            edgeSet.push({ source: sid, target: tid });
        }
        if (_selectedIds.has(sid) && e.target().data('type') === 'author') {
            edgeSet.push({ source: sid, target: tid });
        }
        if (_selectedIds.has(tid) && e.source().data('type') === 'author') {
            edgeSet.push({ source: sid, target: tid });
        }
    });
    return JSON.stringify({ papers: papers, edges: edgeSet });
}

window.addEventListener('resize', () => { if (cy) cy.resize(); });

(function() { PAPER_COLOR = getThemeColors().accent; })();

window.addEventListener('message', function(e) {
    if (!e.data || e.origin !== window.location.origin) return;
    if (e.data.type === 'theme_update') {
        const c = e.data.colors;
        const r = document.documentElement;
        r.style.setProperty('--color-bg',     c.bg);
        r.style.setProperty('--color-panel',  c.panel);
        r.style.setProperty('--color-border', c.border);
        r.style.setProperty('--color-accent', c.accent);
        r.style.setProperty('--color-text',   c.text);
        r.style.setProperty('--color-muted',  c.muted);
        PAPER_COLOR = c.accent;
        if (cy) cy.style(cytoscapeStyle()).update();
    } else if (e.data.type === 'clear_selection') {
        clearSelection();
    } else if (e.data.type === 'set_options') {
        // In-place option change: re-fetch, keeping filters / layout / view.
        if (typeof e.data.excludeSingleAuthors === 'boolean') {
            _excludeSingleAuthors = e.data.excludeSingleAuthors;
        }
        fetchAndLoadGraph({ preserveView: true });
    } else if (e.data.type === 'refresh') {
        fetchAndLoadGraph({ preserveView: true });
    }
});

// ── Data fetch + bootstrap ───────────────────────────────────────────────────
// http(s) = dev (Vite proxy handles /api), tauri: = production app
// (backend at 127.0.0.1:8000), file: = Qt bridge (no fetch — skip).

function _resolveGraphBase() {
    const proto = window.location.protocol;
    if (proto === 'tauri:') return 'http://127.0.0.1:8000';
    if (proto === 'http:' || proto === 'https:') return window.location.origin;
    return null;
}

// Fetch graph data + dropdowns and (re)load the graph. The token guards against
// overlapping calls: only the most recent fetch applies its results.
async function fetchAndLoadGraph(opts = {}) {
    if (!_graphBase) { _notifyHost({ type: 'graph_loaded', ok: false }); return; }
    const token = ++_loadToken;
    const graphUrl = _graphBase + '/api/graph'
        + (_excludeSingleAuthors ? '?exclude_single_authors=true' : '');
    try {
        const [graphData, catData, tagData, projData] = await Promise.all([
            _fetchJson(graphUrl),
            _fetchJson(_graphBase + '/api/categories'),
            _fetchJson(_graphBase + '/api/tags'),
            _fetchJson(_graphBase + '/api/graph/project-options'),
        ]);
        if (token !== _loadToken) return;  // superseded by a newer request
        // Validate every payload before loadGraph destroys the current graph.
        if (!graphData || !Array.isArray(graphData.nodes) || !Array.isArray(graphData.edges)
            || !catData || !tagData || !projData) {
            throw new Error('Malformed graph payload');
        }
        loadGraph(graphData, opts);
        const projects = (projData.projects || []).map(p => ({
            id: p.id,
            name: p.name,
            color: p.color,
            tags: p.tags || [],
        }));
        setFilterOptions(catData.categories || [], tagData.tags || [], projects);
        _notifyHost({ type: 'graph_loaded', ok: true });
    } catch (err) {
        if (token !== _loadToken) return;
        console.error('Graph load failed', err);
        _notifyHost({ type: 'graph_loaded', ok: false });
    }
}

async function _fetchJson(url) {
    const r = await fetch(url);
    if (!r.ok) throw new Error('HTTP ' + r.status + ' for ' + url);
    return r.json();
}

function _notifyHost(msg) {
    window.parent.postMessage(msg, window.location.origin);
}

(function bootstrapWebGraph() {
    _graphBase = _resolveGraphBase();
    if (!_graphBase) return;
    _excludeSingleAuthors =
        new URLSearchParams(window.location.search).get('excludeSingleAuthors') === '1';
    fetchAndLoadGraph({ preserveView: false });
})();
