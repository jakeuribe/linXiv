const PAPER_COLOR     = '#5b8dee';
const AUTHOR_COLOR    = '#e8a838';
const HIGHLIGHT_COLOR = '#ff6b6b';
const DIM_OPACITY     = 0.08;
const FULL_OPACITY    = 1.0;

let cy         = null;
let simulation = null;
let _simNodeById = new Map();  // id → D3 sim node (for O(1) drag lookups)
let _lastOpts  = null;         // re-apply filter after loadGraph

// ── Controls wiring ──────────────────────────────────────────────────────────
const $ = id => document.getElementById(id);

$('controls-toggle').addEventListener('click', () => {
    const body = $('controls-body');
    const collapsed = body.style.display === 'none';
    body.style.display = collapsed ? '' : 'none';
    $('controls-toggle').textContent = collapsed ? '▼' : '▶';
});

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
        n.vx = 0; n.vy = 0;
        n.fx = null; n.fy = null;
    });
    simulation.alpha(1).restart();
});

// ── Graph loading ─────────────────────────────────────────────────────────────

function loadGraph(data) {
    const { nodes, edges } = data;

    if (simulation) { simulation.stop(); simulation = null; }
    if (cy) { cy.destroy(); cy = null; }
    _simNodeById = new Map();
    _lastOpts = null;

    // D3 sim nodes/links — random initial scatter
    const simNodes = nodes.map(n => ({
        id: n.id,
        x: (Math.random() - 0.5) * 800,
        y: (Math.random() - 0.5) * 800,
    }));
    const simLinks = edges.map(e => ({ source: e.source, target: e.target }));
    simNodes.forEach(n => _simNodeById.set(n.id, n));

    // Cytoscape elements — positions set from simNodes, no layout engine
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
        layout:    { name: 'preset' },  // D3 drives positions
        userZoomingEnabled: true,
        userPanningEnabled: true,
        minZoom: 0.05,
        maxZoom: 10,
    });

    cy.fit(undefined, 40);

    // ── Drag: keep D3 and Cytoscape in sync ───────────────────────────────
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

    // ── D3 force simulation ───────────────────────────────────────────────
    const centerStr = parseFloat($('centerForce').value);
    simulation = d3.forceSimulation(simNodes)
        .force('link',      d3.forceLink(simLinks).id(d => d.id)
                              .distance(parseFloat($('linkDistance').value))
                              .strength(parseFloat($('linkStrength').value)))
        .force('charge',    d3.forceManyBody().strength(-parseFloat($('repelForce').value)))
        .force('x',         d3.forceX(0).strength(centerStr))
        .force('y',         d3.forceY(0).strength(centerStr))
        .force('collision', d3.forceCollide(d => 14));

    simulation.on('tick', () => {
        cy.batch(() => {
            simNodes.forEach(d => {
                cy.getElementById(d.id).position({ x: d.x, y: d.y });
            });
        });
    });

    // Re-apply last filter if one was active
    if (_lastOpts) filterGraph(_lastOpts);
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
            style: {
                'border-color': '#ffffff',
                'border-width': 2.5,
            },
        },
    ];
}

// ── Filter ────────────────────────────────────────────────────────────────────

function filterGraph(opts) {
    if (!cy) return;
    _lastOpts = opts;

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
            n.style({ 'opacity': visiblePaperIds.has(n.id()) ? FULL_OPACITY : hiddenOp,
                      'background-color': PAPER_COLOR });
        });
        cy.nodes('[type = "author"]').forEach(n => {
            n.style({ 'opacity': visibleAuthorIds.has(n.id()) ? FULL_OPACITY : hiddenOp });
        });
        cy.edges().forEach(e => {
            const sv = visiblePaperIds.has(e.source().id()) || visibleAuthorIds.has(e.source().id());
            const tv = visiblePaperIds.has(e.target().id()) || visibleAuthorIds.has(e.target().id());
            e.style({ 'opacity': (sv && tv) ? FULL_OPACITY : hiddenOp });
        });
    });
}

// ── Highlight single node ─────────────────────────────────────────────────────

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

window.addEventListener('resize', () => { if (cy) cy.resize(); });
