const svg = d3.select('#graph')
    .attr('width', window.innerWidth)
    .attr('height', window.innerHeight);

const container = svg.append('g');

svg.call(
    d3.zoom()
        .scaleExtent([0.05, 10])
        .on('zoom', (event) => container.attr('transform', event.transform))
);

let simulation = null;

const PAPER_COLOR  = '#5b8dee';
const AUTHOR_COLOR = '#e8a838';

// ── Controls wiring ──
const $ = id => document.getElementById(id);

$('controls-toggle').addEventListener('click', () => {
    const body = $('controls-body');
    const collapsed = body.style.display === 'none';
    body.style.display = collapsed ? '' : 'none';
    $('controls-toggle').textContent = collapsed ? '▼' : '▶';
});

function bindSlider(id, valId, fn) {
    const slider = $(id);
    const display = $(valId);
    slider.addEventListener('input', () => {
        display.textContent = parseFloat(slider.value);
        fn(parseFloat(slider.value));
    });
}

bindSlider('centerForce',   'centerForceVal',   v => {
    if (simulation) {
        simulation.force('x', d3.forceX(window.innerWidth  / 2).strength(v));
        simulation.force('y', d3.forceY(window.innerHeight / 2).strength(v));
        simulation.alpha(0.3).restart();
    }
});
bindSlider('repelForce',    'repelForceVal',    v => {
    if (simulation) simulation.force('charge', d3.forceManyBody().strength(v)).alpha(0.3).restart();
});
bindSlider('linkForce',     'linkForceVal',     v => {
    if (simulation) { simulation.force('link').strength(v); simulation.alpha(0.3).restart(); }
});
bindSlider('linkDistance',  'linkDistanceVal',  v => {
    if (simulation) { simulation.force('link').distance(v); simulation.alpha(0.3).restart(); }
});

function loadGraph(data) {
    const { nodes, edges } = data;

    container.selectAll('*').remove();
    if (simulation) simulation.stop();

    const w = window.innerWidth;
    const h = window.innerHeight;

    const centerStrength = parseFloat($('centerForce').value);

    simulation = d3.forceSimulation(nodes)
        .force('link',      d3.forceLink(edges).id(d => d.id).distance(70))
        .force('charge',    d3.forceManyBody().strength(-180))
        .force('x',         d3.forceX(w / 2).strength(centerStrength))
        .force('y',         d3.forceY(h / 2).strength(centerStrength))
        .force('collision', d3.forceCollide(d => d.type === 'paper' ? 14 : 10));

    const link = container.append('g')
        .attr('class', 'links')
        .selectAll('line')
        .data(edges)
        .join('line');

    // Paper nodes — circles
    const paperNodes = container.append('g')
        .attr('class', 'nodes papers')
        .selectAll('circle')
        .data(nodes.filter(d => d.type === 'paper'))
        .join('circle')
        .attr('r', 10)
        .attr('fill', PAPER_COLOR)
        .call(dragBehavior(simulation));

    paperNodes.append('title').text(d => d.label);

    // Author nodes — diamonds (rotated rect via path)
    const authorNodes = container.append('g')
        .attr('class', 'nodes authors')
        .selectAll('path')
        .data(nodes.filter(d => d.type === 'author'))
        .join('path')
        .attr('d', d3.symbol().type(d3.symbolDiamond).size(120))
        .attr('fill', AUTHOR_COLOR)
        .call(dragBehavior(simulation));

    authorNodes.append('title').text(d => d.label);

    const label = container.append('g')
        .attr('class', 'labels')
        .selectAll('text')
        .data(nodes)
        .join('text')
        .text(d => d.label.length > 35 ? d.label.slice(0, 35) + '…' : d.label)
        .attr('font-size', d => d.type === 'paper' ? '11px' : '10px')
        .attr('fill', d => d.type === 'paper' ? '#aaaacc' : '#c8a060');

    simulation.on('tick', () => {
        link
            .attr('x1', d => d.source.x)
            .attr('y1', d => d.source.y)
            .attr('x2', d => d.target.x)
            .attr('y2', d => d.target.y);

        paperNodes
            .attr('cx', d => d.x)
            .attr('cy', d => d.y);

        authorNodes
            .attr('transform', d => `translate(${d.x},${d.y})`);

        label
            .attr('x', d => d.x + 12)
            .attr('y', d => d.y + 4);
    });
}

function dragBehavior(sim) {
    return d3.drag()
        .on('start', (event, d) => {
            if (!event.active) sim.alphaTarget(0.3).restart();
            d.fx = d.x;
            d.fy = d.y;
        })
        .on('drag', (event, d) => {
            d.fx = event.x;
            d.fy = event.y;
        })
        .on('end', (event, d) => {
            if (!event.active) sim.alphaTarget(0);
            d.fx = null;
            d.fy = null;
        });
}

window.addEventListener('resize', () => {
    const w = window.innerWidth;
    const h = window.innerHeight;
    svg.attr('width', w).attr('height', h);
    if (simulation) {
        simulation.force('x', d3.forceX(w / 2).strength(parseFloat($('centerForce').value)));
        simulation.force('y', d3.forceY(h / 2).strength(parseFloat($('centerForce').value)));
        simulation.restart();
    }
});
