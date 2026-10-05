//! Boykov–Kolmogorov max-flow / min-cut ("An Experimental Comparison of Min-Cut/Max-Flow
//! Algorithms for Energy Minimization in Vision", PAMI 2004), the algorithm GrabCut uses.
//!
//! Two search trees grow from the source and the sink until they touch; the path found is
//! augmented, the nodes it cut off become orphans and are adopted again or freed, and the
//! trees keep growing from where they were. On image grids it is much faster than
//! augmenting-path or push-relabel methods. Capacities are integers so that saturation is
//! exact. Written for Xuan rather than taken from a crate: the maintained Rust max-flow
//! crates are general graph libraries without BK's reuse of search trees, which is what
//! makes grid cuts fast, and the algorithm is a few hundred lines.
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
};

/// Capacity type: integer so that a saturated arc is exactly zero.
pub type Cap = i32;

const NONE: u32 = u32::MAX;
const TERMINAL: u32 = u32::MAX - 1;
const ORPHAN: u32 = u32::MAX - 2;
const INFINITE_DIST: u32 = u32::MAX;

/// Which side of the minimum cut a node is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Source,
    Sink,
}

pub struct Graph {
    // Per node.
    first: Vec<u32>,
    parent: Vec<u32>,
    timestamp: Vec<u32>,
    dist: Vec<u32>,
    sink: Vec<bool>,
    active: Vec<bool>,
    /// Residual terminal capacity: positive towards the source, negative towards the sink.
    terminal: Vec<i64>,
    // Per arc; arcs come in pairs, `a ^ 1` is the reverse of `a`.
    head: Vec<u32>,
    next: Vec<u32>,
    residual: Vec<Cap>,
    flow: i64,
    queue: VecDeque<u32>,
    orphans: VecDeque<u32>,
    time: u32,
}

impl Graph {
    pub fn new(nodes: usize, edges_hint: usize) -> Self {
        Self {
            first: vec![NONE; nodes],
            parent: vec![NONE; nodes],
            timestamp: vec![0; nodes],
            dist: vec![0; nodes],
            sink: vec![false; nodes],
            active: vec![false; nodes],
            terminal: vec![0; nodes],
            head: Vec::with_capacity(edges_hint * 2),
            next: Vec::with_capacity(edges_hint * 2),
            residual: Vec::with_capacity(edges_hint * 2),
            flow: 0,
            queue: VecDeque::new(),
            orphans: VecDeque::new(),
            time: 0,
        }
    }

    pub fn node_count(&self) -> usize {
        self.first.len()
    }

    /// Arcs between nodes, not counting terminal links (each edge is two arcs).
    pub fn edge_count(&self) -> usize {
        self.head.len() / 2
    }

    /// An edge `i → j` with capacity `forward` and `j → i` with `backward`.
    pub fn add_edge(&mut self, i: usize, j: usize, forward: Cap, backward: Cap) {
        debug_assert!(i != j && forward >= 0 && backward >= 0);
        let a = self.head.len() as u32;
        self.head.push(j as u32);
        self.next.push(self.first[i]);
        self.residual.push(forward);
        self.first[i] = a;
        self.head.push(i as u32);
        self.next.push(self.first[j]);
        self.residual.push(backward);
        self.first[j] = a + 1;
    }

    /// Links to the terminals: `source` from the source to `i`, `sink` from `i` to the sink.
    /// Only their difference matters to the cut; the common part flows at once.
    pub fn add_terminal(&mut self, i: usize, source: Cap, sink: Cap) {
        let (mut source, mut sink) = (i64::from(source), i64::from(sink));
        let delta = self.terminal[i];
        if delta > 0 {
            source += delta;
        } else {
            sink -= delta;
        }
        self.flow += source.min(sink);
        self.terminal[i] = source - sink;
    }

    pub fn flow(&self) -> i64 {
        self.flow
    }

    /// The side of node `i` after [`Graph::max_flow`]: the source side is exactly what
    /// the source still reaches through unsaturated arcs.
    pub fn side(&self, i: usize) -> Side {
        if self.parent[i] != NONE && !self.sink[i] {
            Side::Source
        } else {
            Side::Sink
        }
    }

    fn set_active(&mut self, i: u32) {
        if !self.active[i as usize] {
            self.active[i as usize] = true;
            self.queue.push_back(i);
        }
    }

    fn next_active(&mut self) -> Option<u32> {
        while let Some(i) = self.queue.pop_front() {
            self.active[i as usize] = false;
            if self.parent[i as usize] != NONE {
                return Some(i);
            }
        }
        None
    }

    fn orphan_front(&mut self, i: u32) {
        self.parent[i as usize] = ORPHAN;
        self.orphans.push_front(i);
    }

    fn orphan_back(&mut self, i: u32) {
        self.parent[i as usize] = ORPHAN;
        self.orphans.push_back(i);
    }

    /// Computes the maximum flow. Returns `None` when `cancel` is set.
    pub fn max_flow(&mut self, cancel: &AtomicBool) -> Option<i64> {
        for i in 0..self.node_count() {
            let terminal = self.terminal[i];
            if terminal != 0 {
                self.sink[i] = terminal < 0;
                self.parent[i] = TERMINAL;
                self.timestamp[i] = 0;
                self.dist[i] = 1;
                self.set_active(i as u32);
            } else {
                self.parent[i] = NONE;
            }
        }
        let mut current: Option<u32> = None;
        let mut steps = 0u32;
        loop {
            steps = steps.wrapping_add(1);
            if steps.is_multiple_of(4096) && cancel.load(Ordering::Relaxed) {
                return None;
            }
            let i = match current.filter(|&i| self.parent[i as usize] != NONE) {
                Some(i) => i,
                None => match self.next_active() {
                    Some(i) => i,
                    None => break,
                },
            };
            current = None;
            let iu = i as usize;
            let mut meeting = NONE;
            let mut a = self.first[iu];
            if !self.sink[iu] {
                while a != NONE {
                    if self.residual[a as usize] > 0 {
                        let j = self.head[a as usize] as usize;
                        if self.parent[j] == NONE {
                            self.sink[j] = false;
                            self.parent[j] = a ^ 1;
                            self.timestamp[j] = self.timestamp[iu];
                            self.dist[j] = self.dist[iu] + 1;
                            self.set_active(j as u32);
                        } else if self.sink[j] {
                            meeting = a;
                            break;
                        } else if self.timestamp[j] <= self.timestamp[iu]
                            && self.dist[j] > self.dist[iu]
                        {
                            self.parent[j] = a ^ 1;
                            self.timestamp[j] = self.timestamp[iu];
                            self.dist[j] = self.dist[iu] + 1;
                        }
                    }
                    a = self.next[a as usize];
                }
            } else {
                while a != NONE {
                    if self.residual[(a ^ 1) as usize] > 0 {
                        let j = self.head[a as usize] as usize;
                        if self.parent[j] == NONE {
                            self.sink[j] = true;
                            self.parent[j] = a ^ 1;
                            self.timestamp[j] = self.timestamp[iu];
                            self.dist[j] = self.dist[iu] + 1;
                            self.set_active(j as u32);
                        } else if !self.sink[j] {
                            meeting = a ^ 1;
                            break;
                        } else if self.timestamp[j] <= self.timestamp[iu]
                            && self.dist[j] > self.dist[iu]
                        {
                            self.parent[j] = a ^ 1;
                            self.timestamp[j] = self.timestamp[iu];
                            self.dist[j] = self.dist[iu] + 1;
                        }
                    }
                    a = self.next[a as usize];
                }
            }
            self.time += 1;
            if meeting != NONE {
                // `i` may have more paths; look at it again first.
                current = Some(i);
                self.augment(meeting);
                while let Some(orphan) = self.orphans.pop_front() {
                    if self.sink[orphan as usize] {
                        self.adopt_sink(orphan);
                    } else {
                        self.adopt_source(orphan);
                    }
                }
            }
        }
        Some(self.flow)
    }

    /// Pushes the bottleneck along the path through arc `middle` (source tree → sink tree).
    fn augment(&mut self, middle: u32) {
        let mut bottleneck = i64::from(self.residual[middle as usize]);
        // The source tree side.
        let mut i = self.head[(middle ^ 1) as usize] as usize;
        loop {
            let a = self.parent[i];
            if a == TERMINAL {
                break;
            }
            bottleneck = bottleneck.min(i64::from(self.residual[(a ^ 1) as usize]));
            i = self.head[a as usize] as usize;
        }
        bottleneck = bottleneck.min(self.terminal[i]);
        // The sink tree side.
        let mut i = self.head[middle as usize] as usize;
        loop {
            let a = self.parent[i];
            if a == TERMINAL {
                break;
            }
            bottleneck = bottleneck.min(i64::from(self.residual[a as usize]));
            i = self.head[a as usize] as usize;
        }
        bottleneck = bottleneck.min(-self.terminal[i]);
        let b = bottleneck as Cap;
        self.residual[(middle ^ 1) as usize] += b;
        self.residual[middle as usize] -= b;
        let mut i = self.head[(middle ^ 1) as usize] as usize;
        loop {
            let a = self.parent[i];
            if a == TERMINAL {
                break;
            }
            self.residual[a as usize] += b;
            self.residual[(a ^ 1) as usize] -= b;
            if self.residual[(a ^ 1) as usize] == 0 {
                self.orphan_front(i as u32);
            }
            i = self.head[a as usize] as usize;
        }
        self.terminal[i] -= bottleneck;
        if self.terminal[i] == 0 {
            self.orphan_front(i as u32);
        }
        let mut i = self.head[middle as usize] as usize;
        loop {
            let a = self.parent[i];
            if a == TERMINAL {
                break;
            }
            self.residual[(a ^ 1) as usize] += b;
            self.residual[a as usize] -= b;
            if self.residual[a as usize] == 0 {
                self.orphan_front(i as u32);
            }
            i = self.head[a as usize] as usize;
        }
        self.terminal[i] += bottleneck;
        if self.terminal[i] == 0 {
            self.orphan_front(i as u32);
        }
        self.flow += bottleneck;
    }

    /// The distance from `j` to its terminal along tree arcs, or `None` when the path
    /// ends at an orphan. Nodes checked in this round carry the round's timestamp.
    fn origin_distance(&mut self, mut j: usize) -> Option<u32> {
        let mut d = 0u32;
        loop {
            if self.timestamp[j] == self.time {
                return Some(d + self.dist[j]);
            }
            let a = self.parent[j];
            d += 1;
            if a == TERMINAL {
                self.timestamp[j] = self.time;
                self.dist[j] = 1;
                return Some(d);
            }
            if a == ORPHAN {
                return None;
            }
            j = self.head[a as usize] as usize;
        }
    }

    fn mark_path(&mut self, mut j: usize, mut d: u32) {
        while self.timestamp[j] != self.time {
            self.timestamp[j] = self.time;
            self.dist[j] = d;
            d -= 1;
            j = self.head[self.parent[j] as usize] as usize;
        }
    }

    fn adopt_source(&mut self, i: u32) {
        self.adopt(i, false);
    }

    fn adopt_sink(&mut self, i: u32) {
        self.adopt(i, true);
    }

    /// Finds a new parent for orphan `i` in its tree, or frees it.
    fn adopt(&mut self, i: u32, sink: bool) {
        let iu = i as usize;
        let mut best = NONE;
        let mut best_dist = INFINITE_DIST;
        let mut a0 = self.first[iu];
        while a0 != NONE {
            // The arc that would carry flow along the tree: j → i for the source tree,
            // i → j for the sink tree.
            let carries = if sink { a0 } else { a0 ^ 1 };
            if self.residual[carries as usize] > 0 {
                let j = self.head[a0 as usize] as usize;
                if self.sink[j] == sink
                    && self.parent[j] != NONE
                    && let Some(d) = self.origin_distance(j)
                {
                    if d < best_dist {
                        best = a0;
                        best_dist = d;
                    }
                    self.mark_path(j, d);
                }
            }
            a0 = self.next[a0 as usize];
        }
        if best != NONE {
            self.parent[iu] = best;
            self.timestamp[iu] = self.time;
            self.dist[iu] = best_dist + 1;
            return;
        }
        // No parent: `i` becomes free and its children orphans.
        let mut a0 = self.first[iu];
        while a0 != NONE {
            let j = self.head[a0 as usize] as usize;
            let a = self.parent[j];
            if self.sink[j] == sink && a != NONE {
                let carries = if sink { a0 } else { a0 ^ 1 };
                if self.residual[carries as usize] > 0 {
                    self.set_active(j as u32);
                }
                if a != TERMINAL && a != ORPHAN && self.head[a as usize] == i {
                    self.orphan_back(j as u32);
                }
            }
            a0 = self.next[a0 as usize];
        }
        self.parent[iu] = NONE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Edmonds–Karp on a dense matrix, node 0 the source and 1 the sink.
    fn reference(n: usize, capacity: &[Vec<i64>]) -> i64 {
        let mut residual = capacity.to_vec();
        let mut flow = 0;
        loop {
            let mut parent = vec![usize::MAX; n];
            parent[0] = 0;
            let mut queue = VecDeque::from([0]);
            while let Some(u) = queue.pop_front() {
                for v in 0..n {
                    if parent[v] == usize::MAX && residual[u][v] > 0 {
                        parent[v] = u;
                        queue.push_back(v);
                    }
                }
            }
            if parent[1] == usize::MAX {
                return flow;
            }
            let mut bottleneck = i64::MAX;
            let mut v = 1;
            while v != 0 {
                bottleneck = bottleneck.min(residual[parent[v]][v]);
                v = parent[v];
            }
            let mut v = 1;
            while v != 0 {
                residual[parent[v]][v] -= bottleneck;
                residual[v][parent[v]] += bottleneck;
                v = parent[v];
            }
            flow += bottleneck;
        }
    }

    #[test]
    fn matches_edmonds_karp_and_the_cut_is_minimal() {
        let never = AtomicBool::new(false);
        let mut state = 12345u64;
        let mut random = |limit: u64| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) % limit
        };
        for round in 0..300 {
            let nodes = 2 + random(14) as usize;
            let mut graph = Graph::new(nodes, 0);
            // Dense matrix with the source at 0 and the sink at 1, nodes from 2.
            let mut capacity = vec![vec![0i64; nodes + 2]; nodes + 2];
            for i in 0..nodes {
                let (s, t) = (random(20) as Cap, random(20) as Cap);
                let (s, t) = if random(3) == 0 { (s, 0) } else { (s, t) };
                graph.add_terminal(i, s, t);
                capacity[0][i + 2] += i64::from(s);
                capacity[i + 2][1] += i64::from(t);
            }
            for _ in 0..random(40) {
                let (i, j) = (random(nodes as u64) as usize, random(nodes as u64) as usize);
                if i == j {
                    continue;
                }
                let (f, b) = (random(15) as Cap, random(15) as Cap);
                graph.add_edge(i, j, f, b);
                capacity[i + 2][j + 2] += i64::from(f);
                capacity[j + 2][i + 2] += i64::from(b);
            }
            let flow = graph.max_flow(&never).unwrap();
            assert_eq!(flow, reference(nodes + 2, &capacity), "round {round}");
            // The capacity of the cut the sides describe equals the flow.
            let side = |v: usize| match v {
                0 => Side::Source,
                1 => Side::Sink,
                v => graph.side(v - 2),
            };
            let mut cut = 0;
            for (u, row) in capacity.iter().enumerate() {
                for (v, &c) in row.iter().enumerate() {
                    if side(u) == Side::Source && side(v) == Side::Sink {
                        cut += c;
                    }
                }
            }
            assert_eq!(cut, flow, "round {round}");
        }
    }

    #[test]
    fn a_grid_cut_follows_the_weak_edges() {
        // Two columns: the left one tied to the source, the right one to the sink, and a
        // weak seam between columns 3 and 4 of an 8-wide strip.
        let (w, h) = (8, 5);
        let mut graph = Graph::new(w * h, w * h * 2);
        for y in 0..h {
            graph.add_terminal(y * w, 100, 0);
            graph.add_terminal(y * w + w - 1, 0, 100);
            for x in 0..w - 1 {
                let weight = if x == 3 { 1 } else { 10 };
                graph.add_edge(y * w + x, y * w + x + 1, weight, weight);
            }
        }
        assert_eq!(graph.max_flow(&AtomicBool::new(false)), Some(5));
        for y in 0..h {
            for x in 0..w {
                let expected = if x <= 3 { Side::Source } else { Side::Sink };
                assert_eq!(graph.side(y * w + x), expected);
            }
        }
    }

    #[test]
    fn cancelling_stops_the_search() {
        let n = 200 * 200;
        let mut graph = Graph::new(n, n * 2);
        for i in 0..n {
            graph.add_terminal(i, (i % 7) as Cap, (i % 5) as Cap);
            if i % 200 != 199 {
                graph.add_edge(i, i + 1, 3, 3);
            }
            if i + 200 < n {
                graph.add_edge(i, i + 200, 3, 3);
            }
        }
        assert_eq!(graph.max_flow(&AtomicBool::new(true)), None);
    }
}
