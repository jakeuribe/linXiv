const PAPER_COLOR     = '#5b8dee';
const AUTHOR_COLOR    = '#e8a838';
const HIGHLIGHT_COLOR = '#ff6b6b';
const DIM_OPACITY     = 0.08;   // filter dim (isolate / non-matching)
const SEL_DIM_OPACITY = 0.28;   // softer dim for non-selected nodes
const FULL_OPACITY    = 1.0;

let cy          = null;
let simulation  = null;
let _simNodeById = new Map();
let _debounce   = null;
let _selectedIds = new Set();

// Filter state (needed so selection style can layer on top)
let _visiblePaperIds  = null;   // null = no filter active
let _visibleAuthorIds = null;
let _filterIsolate    = false;

// ── Panel collapse wiring ────────────────────────────────────────────────────

document.querySelectorAll('.panel-toggle').forEach(btn => {
    btn.addEventListener('click', () => {
        const body = document.getElementById(btn.dataset.target);
        const collapsed = body.style.display === 'none';
        body.style.display = collapsed ? '' : 'none';
        btn.textContent = collapsed ? '▼' : '▶';
    });
});

// ── Layout sliders ───────────────────────────────────────────────────────────

const $ = id => document.getElementById(id);

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

const _textFilterIds = ['filterCategory', 'filterTag', 'filterDateFrom', 'filterDateTo',
                        'filterTitle', 'filterAuthor'];
const _checkFilterIds = ['showPapers', 'showAuthors', 'filterHasPdf'];

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
        category:     $('filterCategory').value.trim() || null,
        tag:          $('filterTag').value.trim()      || null,
        hasPdf:       $('filterHasPdf').checked,
        dateFrom:     $('filterDateFrom').value.trim() || null,
        dateTo:       $('filterDateTo').value.trim()   || null,
        highlight:    $('filterTitle').value.trim()    || null,
        authorFilter: $('filterAuthor').value.trim()   || null,
        isolate:      $('isolate-btn').classList.contains('active'),
    });
}

// ── Called from Python to populate datalists ─────────────────────────────────

function setFilterOptions(categories, tags) {
    const catList = $('categoryList');
    const tagList = $('tagList');
    catList.innerHTML = categories.map(c => `<option value="${c}">`).join('');
    tagList.innerHTML = tags.map(t => `<option value="${t}">`).join('');
}

// ── Called from Python toolbar "Clear filters" ───────────────────────────────

function clearFilters() {
    _textFilterIds.forEach(id => { $(id).value = ''; });
    _checkFilterIds.forEach(id => { $(id).checked = id !== 'filterHasPdf'; });
    $('isolate-btn').classList.remove('active');
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
    _filterIsolate    = false;

    const simNodes = nodes.map(n => ({
        id: n.id,
        x:  (Math.random() - 0.5) * 800,
        y:  (Math.random() - 0.5) * 800,
    }));
    const simLinks = edges.map(e => ({ source: e.source, target: e.target }));
    simNodes.forEach(n => _simNodeById.set(n.id, n));

    const cyElements = [
        ...nodes.map(n => {
            const sn = _simNodeById.get(n.id);
            return {
                group: 'nodes',
                data: {
                    id:        n.id,
                    label:     n.label,
                    type:      n.type,
                    category:  n.category  || null,
                    tags:      n.tags      || [],
                    has_pdf:   n.has_pdf   || false,
                    published: n.published || null,
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
        category     = null,
        tag          = null,
        hasPdf       = false,
        highlight    = null,
        authorFilter = null,
        dateFrom     = null,
        dateTo       = null,
        isolate      = false,
    } = opts;

    const hlLower   = highlight    ? highlight.toLowerCase()    : null;
    const authLower = authorFilter ? authorFilter.toLowerCase() : null;

    const visiblePaperIds = new Set();
    cy.nodes('[type = "paper"]').forEach(n => {
        if (!showPapers) return;
        const d = n.data();
        if (category && !d.category?.toLowerCase().includes(category.toLowerCase())) return;
        if (tag && !(Array.isArray(d.tags) && d.tags.some(t => t.toLowerCase().includes(tag.toLowerCase())))) return;
        if (hasPdf && !d.has_pdf) return;
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

    _visiblePaperIds  = visiblePaperIds;
    _visibleAuthorIds = visibleAuthorIds;
    _filterIsolate    = isolate;

    _applyAllStyles();

    // Physics: pin non-visible nodes when isolating
    if (simulation) {
        _simNodeById.forEach((sn, id) => {
            const visible = visiblePaperIds.has(id) || visibleAuthorIds.has(id);
            if (isolate && !visible) {
                if (sn.fx == null) { sn.fx = sn.x; sn.fy = sn.y; }
            } else if (!isolate) {
                sn.fx = null; sn.fy = null;
            }
        });
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

    // Author ids connected to any selected paper
    const selAuthorIds = new Set();
    if (anySelected) {
        _selectedIds.forEach(pid => {
            const n = cy.getElementById(pid);
            n.connectedEdges().forEach(e => {
                const other = e.source().id() === pid ? e.target() : e.source();
                if (other.data('type') === 'author') selAuthorIds.add(other.id());
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

        cy.edges().forEach(e => {
            const sid = e.source().id(), tid = e.target().id();
            const srcFilterVis = !filterActive
                || (_visiblePaperIds && _visiblePaperIds.has(sid))
                || (_visibleAuthorIds && _visibleAuthorIds.has(sid));
            const tgtFilterVis = !filterActive
                || (_visiblePaperIds && _visiblePaperIds.has(tid))
                || (_visibleAuthorIds && _visibleAuthorIds.has(tid));

            if (!srcFilterVis || !tgtFilterVis) {
                e.style({ 'opacity': filterHideOp });
            } else if (anySelected) {
                const srcSel = _selectedIds.has(sid) || selAuthorIds.has(sid);
                const tgtSel = _selectedIds.has(tid) || selAuthorIds.has(tid);
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
