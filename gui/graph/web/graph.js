const PAPER_COLOR     = '#5b8dee';
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

// Chip toggle state
let _activeProjectIds = new Set();
let _activeProjTagIds = new Set();
let _projectMap       = new Map();  // id → {name, color, tags[]}

const $ = id => document.getElementById(id);

// ── Panel collapse wiring ────────────────────────────────────────────────────

document.querySelectorAll('.panel-toggle').forEach(btn => {
    btn.addEventListener('click', () => {
        const body = document.getElementById(btn.dataset.target);
        const collapsed = body.style.display === 'none';
        body.style.display = collapsed ? '' : 'none';
        btn.textContent = collapsed ? '▼' : '▶';
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

// ── Selection panel buttons ─────────────────────────────────────────────────

$('select-all-btn').addEventListener('click', () => selectAllPapers());
$('clear-selection-btn').addEventListener('click', () => clearSelection());

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
        projectIds:   _activeProjectIds.size > 0 ? [..._activeProjectIds] : null,
        projTagIds:   _activeProjTagIds.size > 0 ? [..._activeProjTagIds] : null,
    });
}

// ── Called from Python to populate filter chips & datalists ──────────────────

function _makeChip(label, color) {
    const chip = document.createElement('button');
    chip.className = 'chip';
    chip.textContent = label;
    chip.style.setProperty('--chip-color', color);
    return chip;
}

function setFilterOptions(categories, tags, projects) {
    // Category datalist
    $('categoryList').innerHTML = categories.map(c => `<option value="${c}">`).join('');

    // Tag datalist for the logic builder input
    $('tagList').innerHTML = tags.map(t => `<option value="${t}">`).join('');

    // Project chips + project-tag collection
    _activeProjectIds.clear();
    _activeProjTagIds.clear();
    _projectMap.clear();
    const projChips    = $('project-chips');
    const projTagChips = $('project-tag-chips');
    projChips.innerHTML    = '';
    projTagChips.innerHTML = '';

    const allProjTags = new Set();
    (projects || []).forEach(proj => {
        _projectMap.set(proj.id, proj);
        (proj.tags || []).forEach(t => allProjTags.add(t));
        const chip = _makeChip(proj.name, proj.color || '#5b8dee');
        chip.addEventListener('click', () => {
            chip.classList.toggle('active');
            if (chip.classList.contains('active')) _activeProjectIds.add(proj.id);
            else _activeProjectIds.delete(proj.id);
            _applyFilter();
        });
        projChips.appendChild(chip);
    });

    [...allProjTags].sort().forEach(tag => {
        const chip = _makeChip(tag, '#9b59b6');
        chip.addEventListener('click', () => {
            chip.classList.toggle('active');
            if (chip.classList.contains('active')) _activeProjTagIds.add(tag);
            else _activeProjTagIds.delete(tag);
            _applyFilter();
        });
        projTagChips.appendChild(chip);
    });
}

// ── Called from Python toolbar "Clear filters" ───────────────────────────────

function clearFilters() {
    _textFilterIds.forEach(id => { $(id).value = ''; });
    _checkFilterIds.forEach(id => { $(id).checked = id !== 'filterHasPdf'; });
    $('isolate-btn').classList.remove('active');
    _tagRows.length = 0;
    _renderTagRows();
    _activeProjectIds.clear();
    _activeProjTagIds.clear();
    document.querySelectorAll('.chip.active').forEach(c => c.classList.remove('active'));
    _applyFilter();
}

// ── Graph loading ─────────────────────────────────────────────────────────────

function loadGraph(data) {
    const { nodes, edges } = data;

    if (simulation) { simulation.stop(); simulation = null; }
    if (cy) { cy.destroy(); cy = null; }
    _simNodeById = new Map();
    _visiblePaperIds  = null;
    _visibleAuthorIds = null;
    _visibleTagIds    = null;
    _filterIsolate    = false;
    _tagRows.length = 0;
    _renderTagRows();
    _activeProjectIds.clear();
    _activeProjTagIds.clear();
    document.querySelectorAll('.chip.active').forEach(c => c.classList.remove('active'));

    const simNodes = nodes.map(n => ({
        id: n.id,
        x:  (Math.random() - 0.5) * 800,
        y:  (Math.random() - 0.5) * 800,
    }));
    // Store original edge defs before D3 mutates source/target to object refs
    _allEdgeDefs = edges.map(e => ({ source: String(e.source), target: String(e.target) }));
    const simLinks = _allEdgeDefs.map(e => ({ ...e }));
    simNodes.forEach(n => _simNodeById.set(n.id, n));

    const cyElements = [
        ...nodes.map(n => {
            const sn = _simNodeById.get(n.id);
            return {
                group: 'nodes',
                data: {
                    id:          n.id,
                    label:       n.label,
                    type:        n.type,
                    category:    n.category    || null,
                    tags:        n.tags        || [],
                    has_pdf:     n.has_pdf     || false,
                    published:   n.published   || null,
                    project_ids: n.project_ids || [],
                },
                position: { x: sn.x, y: sn.y },
            };
        }),
        ...edges.map(e => ({
            group: 'edges',
            data: { source: e.source, target: e.target },
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
    });

    cy.fit(undefined, 40);

    cy.on('grab', 'node', e => {
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
        if (sn) { sn.fx = null; sn.fy = null; }
        if (simulation) simulation.alphaTarget(0);
    });

    // Click paper node:
    //   Regular click  → set selection to this node alone + navigate
    //   Ctrl/Cmd click → toggle additive (no navigation)
    cy.on('tap', 'node[type = "paper"]', e => {
        const paper_id = e.target.id();
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) {
            _toggleSelection(paper_id);
        } else {
            _selectedIds.clear();
            _selectedIds.add(paper_id);
            _applyAllStyles();
            _notifySelectionChanged();
            console.log('GRAPHVIEW_PAPER_CLICKED:' + paper_id);
        }
    });

    // Right-click paper node → open its Library detail page
    cy.on('cxttap', 'node[type = "paper"]', e => {
        console.log('GRAPHVIEW_PAPER_RIGHT_CLICKED:' + e.target.id());
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
            simNodes.forEach(d => {
                cy.getElementById(d.id).position({ x: d.x, y: d.y });
            });
        });
    });

    _applyFilter();
}

function cytoscapeStyle() {
    return [
        {
            selector: 'node[type = "paper"]',
            style: {
                'shape':            'ellipse',
                'width':            20,
                'height':           20,
                'background-color': PAPER_COLOR,
                'label':            'data(label)',
                'font-size':        '11px',
                'color':            '#aaaacc',
                'font-family':      'Segoe UI, sans-serif',
                'text-valign':      'center',
                'text-halign':      'right',
                'text-margin-x':    8,
                'text-max-width':   '180px',
                'text-wrap':        'ellipsis',
                'border-width':     1.5,
                'border-color':     '#0f0f1a',
            },
        },
        {
            selector: 'node[type = "author"]',
            style: {
                'shape':            'diamond',
                'width':            14,
                'height':           14,
                'background-color': AUTHOR_COLOR,
                'label':            'data(label)',
                'font-size':        '10px',
                'color':            '#c8a060',
                'font-family':      'Segoe UI, sans-serif',
                'text-valign':      'center',
                'text-halign':      'right',
                'text-margin-x':    7,
                'text-max-width':   '140px',
                'text-wrap':        'ellipsis',
            },
        },
        {
            selector: 'node[type = "tag"]',
            style: {
                'shape':            'roundrectangle',
                'width':            'label',
                'height':           18,
                'padding':          '0 6px',
                'background-color': TAG_COLOR,
                'label':            'data(label)',
                'font-size':        '10px',
                'color':            '#d4f0e0',
                'font-family':      'Segoe UI, sans-serif',
                'text-valign':      'center',
                'text-halign':      'center',
                'border-width':     0,
            },
        },
        {
            selector: 'edge',
            style: {
                'width':       1.5,
                'line-color':  '#2a2a4a',
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
            const authorLabels = [];
            n.connectedEdges().forEach(e => {
                const other = e.source().id() === n.id() ? e.target() : e.source();
                if (other.data('type') === 'author') authorLabels.push(other.data('label').toLowerCase());
            });
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

        // Pin non-visible nodes in place; unpin visible ones
        _simNodeById.forEach((sn, id) => {
            if (!visibleNodeIds.has(id)) {
                if (sn.fx == null) { sn.fx = sn.x; sn.fy = sn.y; }
            } else {
                // Only unpin if we own the pin (drag handler sets fx/fy too)
                if (!isolate) { sn.fx = null; sn.fy = null; }
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

// ── Highlight (called from Python when a table row is selected) ───────────────
// Sets the selection to just this one node (or clears if null).

function highlightNode(nodeId) {
    _selectedIds.clear();
    if (nodeId !== null) _selectedIds.add(nodeId);
    _applyAllStyles();
    _notifySelectionChanged();
}

// ── Selection (click to set, Ctrl+click to toggle, Python bulk ops) ──────────

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
    console.log('GRAPHVIEW_SELECTION_COUNT:' + _selectedIds.size);
}

function selectAllPapers() {
    if (!cy) return;
    cy.nodes('[type = "paper"]').forEach(n => {
        if (parseFloat(n.style('opacity')) > DIM_OPACITY) {
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

// When served over HTTP(S) from the FastAPI app, load graph data from the API (file:// keeps Qt bridge).
(function bootstrapWebGraph() {
    if (window.location.protocol !== 'http:' && window.location.protocol !== 'https:') return;
    const base = window.location.origin;
    Promise.all([
        fetch(base + '/api/graph').then(r => r.json()),
        fetch(base + '/api/categories').then(r => r.json()),
        fetch(base + '/api/tags').then(r => r.json()),
        fetch(base + '/api/graph/project-options').then(r => r.json()),
    ]).then(([graphData, catData, tagData, projData]) => {
        loadGraph(graphData);
        const projects = (projData.projects || []).map(p => ({
            id: p.id,
            name: p.name,
            color: p.color,
            tags: p.tags || [],
        }));
        setFilterOptions(catData.categories || [], tagData.tags || [], projects);
    }).catch(e => console.error('Graph bootstrap failed', e));
})();
