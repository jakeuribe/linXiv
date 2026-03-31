const PAPER_COLOR     = '#5b8dee';
const AUTHOR_COLOR    = '#e8a838';
const HIGHLIGHT_COLOR = '#ff6b6b';
const DIM_OPACITY     = 0.08;
const FULL_OPACITY    = 1.0;

const SELECTED_BORDER = '#00e676';

let cy          = null;
let simulation  = null;
let _simNodeById = new Map();
let _debounce   = null;
let _selectedIds = new Set();

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
    _checkFilterIds.forEach(id => { $(id).checked = id !== 'filterHasPdf'; }); // papers+authors default on
    $('isolate-btn').classList.remove('active');
    _applyFilter();
}

// ── Graph loading ─────────────────────────────────────────────────────────────

function loadGraph(data) {
    const { nodes, edges } = data;

    if (simulation) { simulation.stop(); simulation = null; }
    if (cy) { cy.destroy(); cy = null; }
    _simNodeById = new Map();

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

    // Click paper node → navigate or toggle selection (Ctrl+click)
    cy.on('tap', 'node[type = "paper"]', e => {
        const paper_id = e.target.id();
        if (e.originalEvent.ctrlKey || e.originalEvent.metaKey) {
            _toggleSelection(paper_id);
        } else {
            console.log('GRAPHVIEW_PAPER_CLICKED:' + paper_id);
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

    // Re-apply current filter state after load
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
        {
            selector: 'node:selected',
            style: { 'border-color': '#ffffff', 'border-width': 2.5 },
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
    const hiddenOp  = isolate ? 0 : DIM_OPACITY;

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

    cy.batch(() => {
        cy.nodes('[type = "paper"]').forEach(n => {
            n.style({
                'opacity':          showPapers && visiblePaperIds.has(n.id()) ? FULL_OPACITY : (showPapers ? hiddenOp : 0),
                'background-color': PAPER_COLOR,
            });
        });
        cy.nodes('[type = "author"]').forEach(n => {
            n.style({ 'opacity': showAuthors && visibleAuthorIds.has(n.id()) ? FULL_OPACITY : (showAuthors ? hiddenOp : 0) });
        });
        cy.edges().forEach(e => {
            const sv = visiblePaperIds.has(e.source().id()) || visibleAuthorIds.has(e.source().id());
            const tv = visiblePaperIds.has(e.target().id()) || visibleAuthorIds.has(e.target().id());
            e.style({ 'opacity': (sv && tv) ? FULL_OPACITY : hiddenOp });
        });
    });

    // Physics: when isolating, pin non-visible nodes so they don't interfere
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

// ── Highlight single node (called from Python) ────────────────────────────────

function highlightNode(nodeId) {
    if (!cy) return;

    cy.batch(() => {
        cy.nodes('[type = "paper"]').style({ 'opacity': FULL_OPACITY, 'background-color': PAPER_COLOR });
        cy.nodes('[type = "author"]').style({ 'opacity': FULL_OPACITY });
        cy.edges().style({ 'opacity': FULL_OPACITY });
    });

    if (nodeId === null) return;

    const target = cy.getElementById(nodeId);
    if (target.empty()) return;

    const connectedAuthorIds = new Set();
    target.connectedEdges().forEach(e => {
        const other = e.source().id() === nodeId ? e.target() : e.source();
        if (other.data('type') === 'author') connectedAuthorIds.add(other.id());
    });

    cy.batch(() => {
        cy.nodes('[type = "paper"]').forEach(n => {
            n.style({
                'opacity':          n.id() === nodeId ? FULL_OPACITY : DIM_OPACITY,
                'background-color': n.id() === nodeId ? HIGHLIGHT_COLOR : PAPER_COLOR,
            });
        });
        cy.nodes('[type = "author"]').forEach(n => {
            n.style({ 'opacity': connectedAuthorIds.has(n.id()) ? FULL_OPACITY : DIM_OPACITY });
        });
        cy.edges().forEach(e => {
            const connected = e.source().id() === nodeId || e.target().id() === nodeId;
            e.style({ 'opacity': connected ? FULL_OPACITY : DIM_OPACITY });
        });
    });
}

// ── Selection (Ctrl+click to toggle, called from Python for bulk ops) ────────

function _toggleSelection(paperId) {
    if (_selectedIds.has(paperId)) {
        _selectedIds.delete(paperId);
    } else {
        _selectedIds.add(paperId);
    }
    _applySelectionStyle();
    _notifySelectionChanged();
}

function _applySelectionStyle() {
    if (!cy) return;
    cy.batch(() => {
        cy.nodes('[type = "paper"]').forEach(n => {
            if (_selectedIds.has(n.id())) {
                n.style({ 'border-color': SELECTED_BORDER, 'border-width': 3, 'border-style': 'double' });
            } else {
                n.style({ 'border-color': '#0f0f1a', 'border-width': 1.5, 'border-style': 'solid' });
            }
        });
    });
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
    _applySelectionStyle();
    _notifySelectionChanged();
}

function clearSelection() {
    _selectedIds.clear();
    _applySelectionStyle();
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
    // Edges between selected papers
    cy.edges().forEach(e => {
        const sid = e.source().id(), tid = e.target().id();
        if (_selectedIds.has(sid) && _selectedIds.has(tid)) {
            edgeSet.push({ source: sid, target: tid });
        }
        // Also include edges from selected papers to their authors
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
