const PAPER_COLOR     = '#5b8dee';
const AUTHOR_COLOR    = '#e8a838';
const HIGHLIGHT_COLOR = '#ff6b6b';
const DIM_OPACITY     = 0.08;
const FULL_OPACITY    = 1.0;

let cy = null;
let _lastOpts = null;  // re-apply filter after layout reruns

// ── Controls wiring ──────────────────────────────────────────────────────────
const $ = id => document.getElementById(id);

$('controls-toggle').addEventListener('click', () => {
    const body = $('controls-body');
    const collapsed = body.style.display === 'none';
    body.style.display = collapsed ? '' : 'none';
    $('controls-toggle').textContent = collapsed ? '▼' : '▶';
});

function bindSlider(id, valId) {
    $(id).addEventListener('input', () => { $(valId).textContent = $(id).value; });
}
bindSlider('gravity',         'gravityVal');
bindSlider('repulsion',       'repulsionVal');
bindSlider('edgeLength',      'edgeLengthVal');
bindSlider('edgeElasticity',  'edgeElasticityVal');

$('relayout-btn').addEventListener('click', () => {
    if (cy) cy.layout(getLayoutOptions()).run();
});

function getLayoutOptions() {
    const repulsion  = parseFloat($('repulsion').value);
    const edgeLength = parseFloat($('edgeLength').value);
    const elasticity = parseFloat($('edgeElasticity').value);
    return {
        name:           'cose',
        animate:        true,
        refresh:        20,
        fit:            true,
        padding:        40,
        randomize:      false,
        gravity:        parseFloat($('gravity').value),
        nodeRepulsion:  () => repulsion,
        idealEdgeLength:() => edgeLength,
        edgeElasticity: () => elasticity,
        numIter:        1000,
        coolingFactor:  0.99,
        minTemp:        1.0,
        nodeDimensionsIncludeLabels: true,
    };
}

// ── Graph loading ─────────────────────────────────────────────────────────────

function loadGraph(data) {
    const { nodes, edges } = data;

    if (cy) cy.destroy();

    const elements = [
        ...nodes.map(n => ({
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
        })),
        ...edges.map(e => ({
            group: 'edges',
            data: { source: e.source, target: e.target },
        })),
    ];

    cy = cytoscape({
        container: document.getElementById('cy'),
        elements,
        style: cytoscapeStyle(),
        layout: getLayoutOptions(),
        userZoomingEnabled: true,
        userPanningEnabled: true,
        minZoom: 0.05,
        maxZoom: 10,
    });

    // Re-apply last filter after layout settles
    cy.on('layoutstop', () => {
        if (_lastOpts) filterGraph(_lastOpts);
    });
}

function cytoscapeStyle() {
    return [
        {
            selector: 'node[type = "paper"]',
            style: {
                'shape':           'ellipse',
                'width':           20,
                'height':          20,
                'background-color': PAPER_COLOR,
                'label':           'data(label)',
                'font-size':       '11px',
                'color':           '#aaaacc',
                'font-family':     'Segoe UI, sans-serif',
                'text-valign':     'center',
                'text-halign':     'right',
                'text-margin-x':   8,
                'text-max-width':  '180px',
                'text-wrap':       'ellipsis',
                'border-width':    1.5,
                'border-color':    '#0f0f1a',
            },
        },
        {
            selector: 'node[type = "author"]',
            style: {
                'shape':           'diamond',
                'width':           14,
                'height':          14,
                'background-color': AUTHOR_COLOR,
                'label':           'data(label)',
                'font-size':       '10px',
                'color':           '#c8a060',
                'font-family':     'Segoe UI, sans-serif',
                'text-valign':     'center',
                'text-halign':     'right',
                'text-margin-x':   7,
                'text-max-width':  '140px',
                'text-wrap':       'ellipsis',
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

/**
 * filterGraph(opts)
 *
 * opts: {
 *   showAuthors:  bool,
 *   showPapers:   bool,
 *   category:     str|null,   // substring match on node category
 *   tag:          str|null,   // substring match on any tag
 *   hasPdf:       bool,
 *   highlight:    str|null,   // substring match on label
 *   authorFilter: str|null,   // substring match on connected author labels
 *   dateFrom:     str|null,   // ISO date lower bound (inclusive)
 *   dateTo:       str|null,   // ISO date upper bound (inclusive)
 *   isolate:      bool,       // true → hidden nodes are opacity 0, not dimmed
 * }
 */
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

    // ── Visible paper IDs ──
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
                if (other.data('type') === 'author') {
                    authorLabels.push(other.data('label').toLowerCase());
                }
            });
            if (!authorLabels.some(a => a.includes(authLower))) return;
        }

        visiblePaperIds.add(n.id());
    });

    // ── Visible author IDs (connected to a visible paper) ──
    const visibleAuthorIds = new Set();
    if (showAuthors) {
        cy.nodes('[type = "author"]').forEach(a => {
            a.connectedEdges().forEach(e => {
                const other = e.source().id() === a.id() ? e.target() : e.source();
                if (visiblePaperIds.has(other.id())) visibleAuthorIds.add(a.id());
            });
        });
    }

    // ── Apply opacity ──
    cy.nodes('[type = "paper"]').forEach(n => {
        n.style({
            'opacity':           visiblePaperIds.has(n.id()) ? FULL_OPACITY : hiddenOp,
            'background-color':  PAPER_COLOR,
        });
    });
    cy.nodes('[type = "author"]').forEach(n => {
        n.style({ 'opacity': visibleAuthorIds.has(n.id()) ? FULL_OPACITY : hiddenOp });
    });
    cy.edges().forEach(e => {
        const srcVis = visiblePaperIds.has(e.source().id()) || visibleAuthorIds.has(e.source().id());
        const tgtVis = visiblePaperIds.has(e.target().id()) || visibleAuthorIds.has(e.target().id());
        e.style({ 'opacity': (srcVis && tgtVis) ? FULL_OPACITY : hiddenOp });
    });
}

// ── Highlight single node ─────────────────────────────────────────────────────

/**
 * highlightNode(nodeId) — spotlight one paper, dim everything else.
 * Pass null to reset.
 */
function highlightNode(nodeId) {
    if (!cy) return;

    // Reset all to full
    cy.nodes('[type = "paper"]').style({ 'opacity': FULL_OPACITY, 'background-color': PAPER_COLOR });
    cy.nodes('[type = "author"]').style({ 'opacity': FULL_OPACITY });
    cy.edges().style({ 'opacity': FULL_OPACITY });

    if (nodeId === null) return;

    const target = cy.getElementById(nodeId);
    if (target.empty()) return;

    // Collect IDs of directly connected authors
    const connectedAuthorIds = new Set();
    target.connectedEdges().forEach(e => {
        const other = e.source().id() === nodeId ? e.target() : e.source();
        if (other.data('type') === 'author') connectedAuthorIds.add(other.id());
    });

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
}

window.addEventListener('resize', () => { if (cy) cy.resize(); });
