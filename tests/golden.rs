//! Golden-file regression tests plus structural invariant checks.
//!
//! Fixtures under `tests/fixtures/*.json` describe input graphs in a format
//! shared with the Go compatibility harness in `compat/`, which runs the
//! same fixtures through upstream D2's `d2talalayout` engine and asserts the
//! same invariants. This file only asserts tala-rs's own behavior: exact
//! output is pinned against `tests/golden/*.json` (regenerate by running
//! with `TALA_UPDATE_GOLDEN=1`), and layout results must satisfy the
//! structural guarantees TALA promises regardless of exact coordinates.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tala_rs::{Edge, Graph, GraphData, Node, Options, layout};

#[derive(Deserialize)]
struct FixtureNode {
    id: String,
    width: i32,
    height: i32,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    fixed: bool,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
}

#[derive(Deserialize)]
struct FixtureEdge {
    source: String,
    target: String,
}

#[derive(Deserialize)]
struct Fixture {
    seeds: Vec<i64>,
    max_concurrency: usize,
    nodes: Vec<FixtureNode>,
    edges: Vec<FixtureEdge>,
}

#[derive(Serialize)]
struct SnapshotNode {
    id: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Serialize)]
struct SnapshotEdge {
    source: String,
    target: String,
    points: Vec<(i32, i32)>,
}

#[derive(Serialize)]
struct Snapshot {
    nodes: Vec<SnapshotNode>,
    edges: Vec<SnapshotEdge>,
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn load_fixture(path: &Path) -> Fixture {
    let raw = fs::read_to_string(path).unwrap_or_else(|err| panic!("read {path:?}: {err}"));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parse {path:?}: {err}"))
}

fn build_graph(fixture: &Fixture) -> Graph {
    let nodes = fixture
        .nodes
        .iter()
        .map(|n| {
            let mut node = Node::new(n.id.clone(), n.width, n.height);
            if let (Some(x), Some(y)) = (n.x, n.y) {
                node = node.at(x, y);
            }
            node = node.fixed(n.fixed);
            if let Some(parent) = &n.parent {
                node = node.in_parent(parent.clone());
            }
            node
        })
        .collect();

    let edges = fixture
        .edges
        .iter()
        .map(|e| Edge::new(e.source.clone(), e.target.clone()))
        .collect();

    Graph {
        nodes,
        edges,
        data: GraphData::default(),
    }
}

fn run_layout(fixture: &Fixture) -> Graph {
    let mut graph = build_graph(fixture);
    let options = Options {
        seeds: Some(fixture.seeds.clone()),
        max_concurrency: Some(fixture.max_concurrency),
    };
    layout(&mut graph, Some(&options)).expect("layout should succeed for a valid fixture");
    graph
}

fn snapshot(graph: &Graph) -> Snapshot {
    let mut nodes: Vec<SnapshotNode> = graph
        .nodes
        .iter()
        .map(|node| {
            let position = node
                .position
                .unwrap_or_else(|| panic!("layout left node `{}` without a position", node.id));
            SnapshotNode {
                id: node.id.clone(),
                x: position.x,
                y: position.y,
                width: node.width,
                height: node.height,
            }
        })
        .collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));

    let edges = graph
        .edges
        .iter()
        .map(|edge| SnapshotEdge {
            source: edge.source.clone(),
            target: edge.target.clone(),
            points: edge.points.iter().map(|p| (p.x, p.y)).collect(),
        })
        .collect();

    Snapshot { nodes, edges }
}

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(fixtures_dir())
        .expect("read tests/fixtures")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| {
            path.file_stem()
                .expect("fixture has a file stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "expected at least one fixture");
    names
}

#[test]
fn layout_matches_golden_snapshots() {
    let update = std::env::var("TALA_UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    if update {
        fs::create_dir_all(golden_dir()).expect("create tests/golden");
    }

    for name in fixture_names() {
        let fixture = load_fixture(&fixtures_dir().join(format!("{name}.json")));
        let graph = run_layout(&fixture);
        let actual = serde_json::to_string_pretty(&snapshot(&graph)).unwrap();
        let golden_path = golden_dir().join(format!("{name}.json"));

        if update {
            fs::write(&golden_path, format!("{actual}\n")).expect("write golden file");
            continue;
        }

        let expected = fs::read_to_string(&golden_path).unwrap_or_else(|err| {
            panic!(
                "missing golden file {golden_path:?} ({err}); run with TALA_UPDATE_GOLDEN=1 to create it"
            )
        });
        assert_eq!(
            actual.trim_end(),
            expected.trim_end(),
            "layout output for fixture `{name}` no longer matches tests/golden/{name}.json.\n\
             If this change is intentional, re-run with TALA_UPDATE_GOLDEN=1 and review the diff."
        );
    }
}

#[test]
fn layout_satisfies_structural_invariants_for_every_fixture() {
    for name in fixture_names() {
        let fixture = load_fixture(&fixtures_dir().join(format!("{name}.json")));
        let graph = run_layout(&fixture);
        assert_no_unrelated_overlaps(&graph, &name);
        assert_edges_are_orthogonal(&graph, &name);
        assert_children_contained_in_parents(&graph, &name);
    }
}

struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.width
    }

    fn bottom(&self) -> i32 {
        self.y + self.height
    }

    fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    fn contains(&self, other: &Rect) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }
}

fn node_rect(node: &Node) -> Rect {
    let position = node
        .position
        .unwrap_or_else(|| panic!("layout left node `{}` without a position", node.id));
    Rect {
        x: position.x,
        y: position.y,
        width: node.width,
        height: node.height,
    }
}

fn is_ancestor(
    nodes: &[Node],
    id_to_index: &HashMap<&str, usize>,
    ancestor: usize,
    node: usize,
) -> bool {
    let mut current = nodes[node].parent.as_deref();
    while let Some(parent_id) = current {
        let Some(&parent_index) = id_to_index.get(parent_id) else {
            return false;
        };
        if parent_index == ancestor {
            return true;
        }
        current = nodes[parent_index].parent.as_deref();
    }
    false
}

fn assert_no_unrelated_overlaps(graph: &Graph, fixture: &str) {
    let id_to_index: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();

    for left in 0..graph.nodes.len() {
        for right in (left + 1)..graph.nodes.len() {
            if is_ancestor(&graph.nodes, &id_to_index, left, right)
                || is_ancestor(&graph.nodes, &id_to_index, right, left)
            {
                continue;
            }
            let left_rect = node_rect(&graph.nodes[left]);
            let right_rect = node_rect(&graph.nodes[right]);
            assert!(
                !left_rect.intersects(&right_rect),
                "fixture `{fixture}`: unrelated nodes `{}` and `{}` overlap",
                graph.nodes[left].id,
                graph.nodes[right].id
            );
        }
    }
}

fn assert_edges_are_orthogonal(graph: &Graph, fixture: &str) {
    for edge in &graph.edges {
        assert!(
            edge.points.len() >= 2,
            "fixture `{fixture}`: edge `{}`->`{}` has fewer than two route points",
            edge.source,
            edge.target
        );
        for segment in edge.points.windows(2) {
            assert!(
                segment[0].x == segment[1].x || segment[0].y == segment[1].y,
                "fixture `{fixture}`: edge `{}`->`{}` has a non-orthogonal segment",
                edge.source,
                edge.target
            );
        }
    }
}

fn assert_children_contained_in_parents(graph: &Graph, fixture: &str) {
    let id_to_index: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();

    for node in &graph.nodes {
        let Some(parent_id) = &node.parent else {
            continue;
        };
        let parent_index = id_to_index[parent_id.as_str()];
        let parent_rect = node_rect(&graph.nodes[parent_index]);
        let child_rect = node_rect(node);
        assert!(
            parent_rect.contains(&child_rect),
            "fixture `{fixture}`: child `{}` is not contained within parent `{}`",
            node.id,
            parent_id
        );
    }
}
