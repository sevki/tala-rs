use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

const DEFAULT_SEEDS: [i64; 3] = [1, 2, 3];
const MAX_SEEDS: usize = 16;
const MAX_SEED_ENTRIES: usize = MAX_SEEDS * 4;
const DEFAULT_MAX_CONCURRENCY: usize = 4;
const PADDING: i32 = 32;
const GAP: i32 = 24;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    EmptySeeds,
    TooManySeedEntries { limit: usize },
    TooManyUniqueSeeds { limit: usize },
    InvalidMaxConcurrency { max: usize },
    DuplicateNodeId(String),
    MissingNode(String),
    MissingEndpoint { edge_index: usize, node_id: String },
    InvalidNodeDimensions(String),
    FixedNodeWithoutPosition(String),
    CycleDetected(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySeeds => write!(f, "tala requires at least one seed"),
            Self::TooManySeedEntries { limit } => {
                write!(f, "tala accepts at most {limit} seed entries")
            }
            Self::TooManyUniqueSeeds { limit } => {
                write!(f, "tala supports at most {limit} unique seeds")
            }
            Self::InvalidMaxConcurrency { max } => {
                write!(
                    f,
                    "tala max concurrency must be between 1 and {max}, or omitted for the default"
                )
            }
            Self::DuplicateNodeId(id) => write!(f, "duplicate node id `{id}`"),
            Self::MissingNode(id) => write!(f, "missing node `{id}`"),
            Self::MissingEndpoint {
                edge_index,
                node_id,
            } => write!(f, "edge {edge_index} references missing node `{node_id}`"),
            Self::InvalidNodeDimensions(id) => {
                write!(f, "node `{id}` must have positive width and height")
            }
            Self::FixedNodeWithoutPosition(id) => {
                write!(f, "fixed node `{id}` must have an explicit position")
            }
            Self::CycleDetected(id) => write!(f, "cycle detected in container hierarchy at `{id}`"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub seeds: Option<Vec<i64>>,
    pub max_concurrency: Option<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            seeds: Some(DEFAULT_SEEDS.to_vec()),
            max_concurrency: Some(default_max_concurrency()),
        }
    }
}

#[must_use]
pub fn default_options() -> Options {
    Options::default()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphData {
    pub tala_seeds: Option<Vec<i64>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub width: i32,
    pub height: i32,
    pub position: Option<Point>,
    pub fixed: bool,
    pub parent: Option<String>,
}

impl Node {
    #[must_use]
    pub fn new(id: impl Into<String>, width: i32, height: i32) -> Self {
        Self {
            id: id.into(),
            width,
            height,
            position: None,
            fixed: false,
            parent: None,
        }
    }

    #[must_use]
    pub fn at(mut self, x: i32, y: i32) -> Self {
        self.position = Some(Point { x, y });
        self
    }

    #[must_use]
    pub fn fixed(mut self, fixed: bool) -> Self {
        self.fixed = fixed;
        self
    }

    #[must_use]
    pub fn in_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub points: Vec<Point>,
}

impl Edge {
    #[must_use]
    pub fn new(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            points: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub data: GraphData,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Rect {
    fn right(self) -> i32 {
        self.x + self.width
    }

    fn bottom(self) -> i32 {
        self.y + self.height
    }

    fn center(self) -> Point {
        Point {
            x: self.x + self.width / 2,
            y: self.y + self.height / 2,
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Size {
    width: i32,
    height: i32,
}

#[derive(Clone, Debug, Default)]
struct LayoutCache {
    subtree_sizes: HashMap<usize, Size>,
    child_offsets: HashMap<usize, Vec<(usize, Point)>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    overlap_penalty: usize,
    intersection_penalty: usize,
    turns: usize,
    area: i64,
}

#[derive(Clone, Debug)]
struct AttemptResult {
    graph: Graph,
    score: Score,
}

pub fn default_layout(graph: &mut Graph) -> Result<()> {
    layout(graph, None)
}

pub fn layout(graph: &mut Graph, options: Option<&Options>) -> Result<()> {
    let validated = graph.clone();
    let plan = layout_plan(&validated, options)?;
    validate_graph(&validated)?;

    let attempt = run_seed_attempts(validated, &plan.seeds, plan.max_concurrency)?;
    *graph = attempt.graph;
    Ok(())
}

pub fn route_edges(graph: &mut Graph) -> Result<()> {
    let metadata = validate_graph(graph)?;
    reroute_all_edges(graph, &metadata, 0);
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LayoutPlan {
    seeds: Vec<i64>,
    max_concurrency: usize,
}

#[derive(Clone, Debug)]
struct GraphMetadata {
    id_to_index: HashMap<String, usize>,
    parent_indices: Vec<Option<usize>>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

fn default_max_concurrency() -> usize {
    thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, DEFAULT_MAX_CONCURRENCY)
}

fn layout_plan(graph: &Graph, options: Option<&Options>) -> Result<LayoutPlan> {
    let defaults = Options::default();
    let option_seeds = options
        .and_then(|value| value.seeds.clone())
        .or_else(|| defaults.seeds.clone())
        .unwrap_or_else(|| DEFAULT_SEEDS.to_vec());

    let seeds = graph.data.tala_seeds.clone().unwrap_or(option_seeds);
    let seeds = normalize_seeds(seeds)?;

    let raw_concurrency = options
        .and_then(|value| value.max_concurrency)
        .or(defaults.max_concurrency)
        .unwrap_or_else(default_max_concurrency);
    if raw_concurrency == 0 || raw_concurrency > MAX_SEEDS {
        return Err(Error::InvalidMaxConcurrency { max: MAX_SEEDS });
    }

    Ok(LayoutPlan {
        max_concurrency: raw_concurrency.min(seeds.len()).max(1),
        seeds,
    })
}

fn normalize_seeds(seeds: Vec<i64>) -> Result<Vec<i64>> {
    if seeds.is_empty() {
        return Err(Error::EmptySeeds);
    }
    if seeds.len() > MAX_SEED_ENTRIES {
        return Err(Error::TooManySeedEntries {
            limit: MAX_SEED_ENTRIES,
        });
    }

    let mut unique = Vec::with_capacity(seeds.len().min(MAX_SEEDS));
    let mut seen = HashSet::with_capacity(seeds.len().min(MAX_SEEDS));
    for seed in seeds {
        if seen.insert(seed) {
            unique.push(seed);
            if unique.len() > MAX_SEEDS {
                return Err(Error::TooManyUniqueSeeds { limit: MAX_SEEDS });
            }
        }
    }
    Ok(unique)
}

fn validate_graph(graph: &Graph) -> Result<GraphMetadata> {
    let mut id_to_index = HashMap::with_capacity(graph.nodes.len());
    for (index, node) in graph.nodes.iter().enumerate() {
        if node.width <= 0 || node.height <= 0 {
            return Err(Error::InvalidNodeDimensions(node.id.clone()));
        }
        if node.fixed && node.position.is_none() {
            return Err(Error::FixedNodeWithoutPosition(node.id.clone()));
        }
        if id_to_index.insert(node.id.clone(), index).is_some() {
            return Err(Error::DuplicateNodeId(node.id.clone()));
        }
    }

    let mut parent_indices = vec![None; graph.nodes.len()];
    let mut children = vec![Vec::new(); graph.nodes.len()];
    let mut roots = Vec::new();
    for (index, node) in graph.nodes.iter().enumerate() {
        if let Some(parent) = node.parent.as_ref() {
            let parent_index = id_to_index
                .get(parent)
                .copied()
                .ok_or_else(|| Error::MissingNode(parent.clone()))?;
            parent_indices[index] = Some(parent_index);
            children[parent_index].push(index);
        } else {
            roots.push(index);
        }
    }

    for start in 0..graph.nodes.len() {
        let mut seen = HashSet::new();
        let mut current = Some(start);
        while let Some(index) = current {
            if !seen.insert(index) {
                return Err(Error::CycleDetected(graph.nodes[start].id.clone()));
            }
            current = parent_indices[index];
        }
    }

    for (index, edge) in graph.edges.iter().enumerate() {
        if !id_to_index.contains_key(&edge.source) {
            return Err(Error::MissingEndpoint {
                edge_index: index,
                node_id: edge.source.clone(),
            });
        }
        if !id_to_index.contains_key(&edge.target) {
            return Err(Error::MissingEndpoint {
                edge_index: index,
                node_id: edge.target.clone(),
            });
        }
    }

    Ok(GraphMetadata {
        id_to_index,
        parent_indices,
        children,
        roots,
    })
}

fn run_seed_attempts(graph: Graph, seeds: &[i64], max_concurrency: usize) -> Result<AttemptResult> {
    let worker_count = max_concurrency.min(seeds.len()).max(1);
    let graph = Arc::new(graph);
    // `seeds` is borrowed, but the queue is shared with `'static` worker
    // threads below, so an owned copy is required here.
    #[allow(clippy::unnecessary_to_owned)]
    let jobs = Arc::new(Mutex::new(seeds.to_vec().into_iter().enumerate()));
    let (tx, rx) = mpsc::channel();

    let mut handles = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let graph = Arc::clone(&graph);
        let jobs = Arc::clone(&jobs);
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            loop {
                let next = {
                    let mut guard = jobs.lock().expect("seed queue lock poisoned");
                    guard.next()
                };
                let Some((index, seed)) = next else {
                    break;
                };
                let result = layout_seed((*graph).clone(), seed);
                if tx.send((index, result)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);

    let mut best: Option<(usize, AttemptResult)> = None;
    let mut first_error: Option<Error> = None;
    for (index, result) in rx {
        match result {
            Ok(attempt) => {
                let replace = match &best {
                    None => true,
                    Some((best_index, best_attempt)) => {
                        attempt.score < best_attempt.score
                            || (attempt.score == best_attempt.score && index > *best_index)
                    }
                };
                if replace {
                    best = Some((index, attempt));
                }
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    for handle in handles {
        let _ = handle.join();
    }

    best.map(|(_, attempt)| attempt)
        .ok_or_else(|| first_error.unwrap_or(Error::EmptySeeds))
}

fn layout_seed(mut graph: Graph, seed: i64) -> Result<AttemptResult> {
    let metadata = validate_graph(&graph)?;
    let mut cache = LayoutCache::default();
    for &root in &metadata.roots {
        measure_subtree(&mut graph, root, &metadata, seed, &mut cache);
    }
    let root_positions = arrange_siblings(&graph, &metadata.roots, None, &cache, seed);
    for &root in &metadata.roots {
        let origin = root_positions
            .get(&root)
            .copied()
            .unwrap_or_else(|| graph.nodes[root].position.unwrap_or_default());
        place_subtree(&mut graph, root, origin, &metadata, &cache);
    }
    reroute_all_edges(&mut graph, &metadata, seed);
    let score = score_graph(&graph, &metadata);
    Ok(AttemptResult { graph, score })
}

fn measure_subtree(
    graph: &mut Graph,
    index: usize,
    metadata: &GraphMetadata,
    seed: i64,
    cache: &mut LayoutCache,
) -> Size {
    if let Some(size) = cache.subtree_sizes.get(&index).copied() {
        return size;
    }

    let base = Size {
        width: graph.nodes[index].width,
        height: graph.nodes[index].height,
    };
    let children = &metadata.children[index];
    if children.is_empty() {
        cache.subtree_sizes.insert(index, base);
        return base;
    }

    for &child in children {
        measure_subtree(
            graph,
            child,
            metadata,
            seed ^ mix64(index as u64) as i64,
            cache,
        );
    }

    let offsets = arrange_siblings(graph, children, Some(index), cache, seed);
    let mut max_right = 0;
    let mut max_bottom = 0;
    let mut stored_offsets = Vec::with_capacity(children.len());
    for &child in children {
        let offset = offsets.get(&child).copied().unwrap_or_default();
        let child_size = cache.subtree_sizes[&child];
        max_right = max_right.max(offset.x + child_size.width);
        max_bottom = max_bottom.max(offset.y + child_size.height);
        stored_offsets.push((child, offset));
    }
    cache.child_offsets.insert(index, stored_offsets);

    let size = Size {
        width: base.width.max(max_right + PADDING * 2),
        height: base.height.max(max_bottom + PADDING * 2),
    };
    graph.nodes[index].width = size.width;
    graph.nodes[index].height = size.height;
    cache.subtree_sizes.insert(index, size);
    size
}

fn arrange_siblings(
    graph: &Graph,
    siblings: &[usize],
    parent: Option<usize>,
    cache: &LayoutCache,
    seed: i64,
) -> HashMap<usize, Point> {
    let mut positions = HashMap::with_capacity(siblings.len());
    if siblings.is_empty() {
        return positions;
    }

    let mut max_width = 0;
    let mut max_height = 0;
    for &index in siblings {
        let size = cache.subtree_sizes.get(&index).copied().unwrap_or(Size {
            width: graph.nodes[index].width,
            height: graph.nodes[index].height,
        });
        max_width = max_width.max(size.width);
        max_height = max_height.max(size.height);
    }
    let cell_width = max_width + GAP;
    let cell_height = max_height + GAP;
    let columns = choose_column_count(siblings.len(), seed, parent);

    let mut ordered = siblings.to_vec();
    ordered.sort_by_key(|&index| seeded_order_key(seed, &graph.nodes[index].id, index));

    let mut occupied = Vec::with_capacity(siblings.len());
    for &index in siblings {
        if graph.nodes[index].fixed
            && let Some(position) = graph.nodes[index].position
        {
            positions.insert(index, position);
            let size = cache.subtree_sizes.get(&index).copied().unwrap_or(Size {
                width: graph.nodes[index].width,
                height: graph.nodes[index].height,
            });
            occupied.push(Rect {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
            });
        }
    }

    let mut next_slot = 0usize;
    for index in ordered {
        if positions.contains_key(&index) {
            continue;
        }
        let size = cache.subtree_sizes.get(&index).copied().unwrap_or(Size {
            width: graph.nodes[index].width,
            height: graph.nodes[index].height,
        });
        loop {
            let column = next_slot % columns;
            let row = next_slot / columns;
            let candidate = Point {
                x: column as i32 * cell_width,
                y: row as i32 * cell_height,
            };
            next_slot += 1;
            let rect = Rect {
                x: candidate.x,
                y: candidate.y,
                width: size.width,
                height: size.height,
            };
            if occupied.iter().all(|existing| !existing.intersects(rect)) {
                positions.insert(index, candidate);
                occupied.push(rect);
                break;
            }
        }
    }

    positions
}

fn choose_column_count(node_count: usize, seed: i64, parent: Option<usize>) -> usize {
    if node_count <= 1 {
        return 1;
    }

    let max_columns = node_count;
    let preferred =
        ((mix64(seed as u64 ^ parent.unwrap_or_default() as u64) as usize) % max_columns) + 1;
    preferred.min(node_count).max(1)
}

fn place_subtree(
    graph: &mut Graph,
    index: usize,
    origin: Point,
    metadata: &GraphMetadata,
    cache: &LayoutCache,
) {
    graph.nodes[index].position = Some(origin);
    if let Some(offsets) = cache.child_offsets.get(&index) {
        for &(child, offset) in offsets {
            place_subtree(
                graph,
                child,
                Point {
                    x: origin.x + PADDING + offset.x,
                    y: origin.y + PADDING + offset.y,
                },
                metadata,
                cache,
            );
        }
    } else {
        let _ = metadata;
    }
}

fn reroute_all_edges(graph: &mut Graph, metadata: &GraphMetadata, seed: i64) {
    let routes: Vec<Vec<Point>> = graph
        .edges
        .iter()
        .map(|edge| {
            let source = metadata.id_to_index[&edge.source];
            let target = metadata.id_to_index[&edge.target];
            let source_rect = node_rect(&graph.nodes[source]);
            let target_rect = node_rect(&graph.nodes[target]);
            route_edge(
                source_rect,
                target_rect,
                graph,
                metadata,
                source,
                target,
                seed,
            )
        })
        .collect();

    for (edge, points) in graph.edges.iter_mut().zip(routes) {
        edge.points = points;
    }
}

fn route_edge(
    source_rect: Rect,
    target_rect: Rect,
    graph: &Graph,
    metadata: &GraphMetadata,
    source_index: usize,
    target_index: usize,
    seed: i64,
) -> Vec<Point> {
    let source = source_rect.center();
    let target = target_rect.center();
    if source.x == target.x || source.y == target.y {
        return vec![source, target];
    }

    let hv = simplify_path(vec![
        source,
        Point {
            x: target.x,
            y: source.y,
        },
        target,
    ]);
    let vh = simplify_path(vec![
        source,
        Point {
            x: source.x,
            y: target.y,
        },
        target,
    ]);

    let hv_penalty = route_penalty(&hv, graph, metadata, source_index, target_index);
    let vh_penalty = route_penalty(&vh, graph, metadata, source_index, target_index);
    if hv_penalty < vh_penalty {
        hv
    } else if vh_penalty < hv_penalty {
        vh
    } else if mix64(seed as u64) & 1 == 0 {
        hv
    } else {
        vh
    }
}

fn simplify_path(points: Vec<Point>) -> Vec<Point> {
    let mut simplified = Vec::with_capacity(points.len());
    for point in points {
        if simplified.last().copied() == Some(point) {
            continue;
        }
        if simplified.len() >= 2 {
            let second_last = simplified[simplified.len() - 2];
            let last = simplified[simplified.len() - 1];
            if (second_last.x == last.x && last.x == point.x)
                || (second_last.y == last.y && last.y == point.y)
            {
                let len = simplified.len();
                simplified[len - 1] = point;
                continue;
            }
        }
        simplified.push(point);
    }
    simplified
}

fn route_penalty(
    points: &[Point],
    graph: &Graph,
    metadata: &GraphMetadata,
    source_index: usize,
    target_index: usize,
) -> usize {
    graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != source_index && *index != target_index)
        .filter(|(index, _)| {
            !is_ancestor(metadata, *index, source_index)
                && !is_ancestor(metadata, *index, target_index)
        })
        .map(|(_, node)| {
            let rect = node_rect(node);
            points
                .windows(2)
                .filter(|segment| segment_intersects_rect(segment[0], segment[1], rect))
                .count()
        })
        .sum()
}

fn score_graph(graph: &Graph, metadata: &GraphMetadata) -> Score {
    let mut overlap_penalty = 0;
    for left in 0..graph.nodes.len() {
        for right in (left + 1)..graph.nodes.len() {
            if is_ancestor(metadata, left, right) || is_ancestor(metadata, right, left) {
                continue;
            }
            if node_rect(&graph.nodes[left]).intersects(node_rect(&graph.nodes[right])) {
                overlap_penalty += 1;
            }
        }
    }

    let mut intersection_penalty = 0;
    let turns = graph
        .edges
        .iter()
        .map(|edge| {
            intersection_penalty += route_penalty(
                &edge.points,
                graph,
                metadata,
                metadata.id_to_index[&edge.source],
                metadata.id_to_index[&edge.target],
            );
            edge.points.len().saturating_sub(2)
        })
        .sum();

    let area = bounding_area(graph);
    Score {
        overlap_penalty,
        intersection_penalty,
        turns,
        area,
    }
}

fn bounding_area(graph: &Graph) -> i64 {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for node in &graph.nodes {
        let rect = node_rect(node);
        min_x = min_x.min(rect.x);
        min_y = min_y.min(rect.y);
        max_x = max_x.max(rect.right());
        max_y = max_y.max(rect.bottom());
    }
    if graph.nodes.is_empty() {
        0
    } else {
        i64::from(max_x - min_x) * i64::from(max_y - min_y)
    }
}

fn node_rect(node: &Node) -> Rect {
    let position = node.position.unwrap_or_default();
    Rect {
        x: position.x,
        y: position.y,
        width: node.width,
        height: node.height,
    }
}

fn is_ancestor(metadata: &GraphMetadata, ancestor: usize, node: usize) -> bool {
    let mut current = metadata.parent_indices[node];
    while let Some(index) = current {
        if index == ancestor {
            return true;
        }
        current = metadata.parent_indices[index];
    }
    false
}

fn segment_intersects_rect(start: Point, end: Point, rect: Rect) -> bool {
    if start.x == end.x {
        let x = start.x;
        let (min_y, max_y) = if start.y <= end.y {
            (start.y, end.y)
        } else {
            (end.y, start.y)
        };
        x > rect.x && x < rect.right() && max_y > rect.y && min_y < rect.bottom()
    } else if start.y == end.y {
        let y = start.y;
        let (min_x, max_x) = if start.x <= end.x {
            (start.x, end.x)
        } else {
            (end.x, start.x)
        };
        y > rect.y && y < rect.bottom() && max_x > rect.x && min_x < rect.right()
    } else {
        false
    }
}

fn seeded_order_key(seed: i64, id: &str, index: usize) -> u64 {
    mix64(seed as u64 ^ hash_string(id) ^ index as u64)
}

fn hash_string(value: &str) -> u64 {
    value
        .as_bytes()
        .iter()
        .fold(1_469_598_103_934_665_603_u64, |acc, byte| {
            acc.wrapping_mul(1_099_511_628_211)
                .wrapping_add(u64::from(*byte))
        })
}

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_seeds_deduplicates_in_order() {
        let seeds = normalize_seeds(vec![7, 7, 1, 2, 1]).unwrap();
        assert_eq!(seeds, vec![7, 1, 2]);
    }

    #[test]
    fn layout_uses_graph_seed_data_over_options() {
        let graph = Graph {
            nodes: vec![
                Node::new("a", 80, 40),
                Node::new("b", 80, 40),
                Node::new("c", 80, 40),
            ],
            edges: vec![],
            data: GraphData {
                tala_seeds: Some(vec![2]),
            },
        };
        let options = Options {
            seeds: Some(vec![1]),
            max_concurrency: Some(1),
        };

        let plan = layout_plan(&graph, Some(&options)).unwrap();
        assert_eq!(plan.seeds, vec![2]);
    }

    #[test]
    fn layout_preserves_fixed_positions() {
        let mut graph = Graph {
            nodes: vec![
                Node::new("fixed", 80, 40).at(300, 200).fixed(true),
                Node::new("free", 80, 40),
            ],
            edges: vec![Edge::new("fixed", "free")],
            data: GraphData::default(),
        };

        default_layout(&mut graph).unwrap();

        let fixed = graph.nodes.iter().find(|node| node.id == "fixed").unwrap();
        assert_eq!(fixed.position, Some(Point { x: 300, y: 200 }));
    }

    #[test]
    fn layout_sizes_containers_to_fit_children() {
        let mut graph = Graph {
            nodes: vec![
                Node::new("container", 100, 60),
                Node::new("child-a", 90, 40).in_parent("container"),
                Node::new("child-b", 90, 40).in_parent("container"),
            ],
            edges: vec![],
            data: GraphData::default(),
        };

        layout(&mut graph, None).unwrap();

        let container = graph
            .nodes
            .iter()
            .find(|node| node.id == "container")
            .unwrap();
        let container_rect = node_rect(container);
        assert!(container.width > 100);
        assert!(container.height > 60);
        for child in graph
            .nodes
            .iter()
            .filter(|node| node.parent.as_deref() == Some("container"))
        {
            let child_rect = node_rect(child);
            assert!(child_rect.x >= container_rect.x + PADDING);
            assert!(child_rect.y >= container_rect.y + PADDING);
            assert!(child_rect.right() <= container_rect.right() - PADDING);
            assert!(child_rect.bottom() <= container_rect.bottom() - PADDING);
        }
    }

    #[test]
    fn route_edges_produces_orthogonal_segments() {
        let mut graph = Graph {
            nodes: vec![
                Node::new("a", 80, 40).at(0, 0).fixed(true),
                Node::new("b", 80, 40).at(200, 100).fixed(true),
            ],
            edges: vec![Edge::new("a", "b")],
            data: GraphData::default(),
        };

        route_edges(&mut graph).unwrap();

        let edge = &graph.edges[0];
        assert!(
            edge.points
                .windows(2)
                .all(|segment| { segment[0].x == segment[1].x || segment[0].y == segment[1].y })
        );
    }

    #[test]
    fn later_seed_wins_exact_tie() {
        let graph = Graph {
            nodes: vec![Node::new("a", 80, 40)],
            edges: vec![],
            data: GraphData::default(),
        };

        let result = run_seed_attempts(graph, &[1, 2], 1).unwrap();
        assert_eq!(result.graph.nodes[0].position, Some(Point { x: 0, y: 0 }));
    }

    #[test]
    fn rejects_invalid_concurrency() {
        let graph = Graph::default();
        let options = Options {
            seeds: Some(vec![1]),
            max_concurrency: Some(0),
        };
        assert_eq!(
            layout_plan(&graph, Some(&options)).unwrap_err(),
            Error::InvalidMaxConcurrency { max: MAX_SEEDS }
        );
    }
}
