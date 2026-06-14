"""Graph nodes/edges matching the desktop GraphPage augmentation (tags + projects)."""

from __future__ import annotations

from storage.config.queries import Q, list_project_tags_bulk
from storage.db import get_graph_data
from storage.projects import Project, Status, filter_projects, color_to_hex

_DEFAULT_PROJECT_COLOR = "#5b8dee"


def get_augmented_graph_data(exclude_single_authors: bool = False) -> dict[str, list[dict]]:
    all_nodes, edges = get_graph_data(exclude_single_authors=exclude_single_authors)  # paper + author nodes

    paper_to_projects: dict[int, list[int]] = {}
    for proj in filter_projects(Q("STATUS = ?", Status.ACTIVE), load_sources=True):
        if proj.id is not None:
            for sfk in proj.source_fks:
                paper_to_projects.setdefault(sfk, []).append(proj.id)

    tag_labels: dict[str, str] = {}
    seen_tag_edges: set[tuple[int, str]] = set()
    tag_edges: list[dict] = []
    output_nodes: list[dict] = []
    for node in all_nodes:
        if node.get("type") != "paper":
            output_nodes.append(node)
            continue
        annotated = dict(node)
        annotated["project_ids"] = sorted(set(paper_to_projects.get(annotated["id"], [])))
        output_nodes.append(annotated)
        for raw_tag in (node.get("tags") or []):
            if not isinstance(raw_tag, str):
                continue
            tag = raw_tag.strip()
            if not tag:
                continue
            tag_node_id = f"tag::{tag.lower()}"
            tag_labels.setdefault(tag_node_id, tag)
            pair = (node["id"], tag_node_id)
            if pair not in seen_tag_edges:
                seen_tag_edges.add(pair)
                tag_edges.append({"source": node["id"], "target": tag_node_id})

    tag_nodes = [{"id": tid, "label": tag_labels[tid], "type": "tag"} for tid in sorted(tag_labels)]

    return {"nodes": output_nodes + tag_nodes, "edges": edges + tag_edges}


def project_filter_options() -> list[dict]:
    """Project chips for the graph (same shape as desktop GraphPage._load_dropdowns)."""
    active: list[tuple[int, Project]] = []
    for p in filter_projects(Q("STATUS = ?", Status.ACTIVE), load_sources=False):
        if p.id is not None:
            active.append((p.id, p))
    active.sort(key=lambda t: t[0])
    if not active:
        return []
    project_ids = [pid for pid, _ in active]
    tags_by_project = list_project_tags_bulk(project_ids)
    return [
        {
            "id": pid,
            "name": p.name,
            "color": color_to_hex(p.color) if p.color is not None else _DEFAULT_PROJECT_COLOR,
            "tags": tags_by_project.get(pid, []),
        }
        for pid, p in active
    ]
