//! Node-based grading (the Color page's node graph).
//!
//! A clip's grade is a small directed acyclic graph: an **Input** node (the clip's picture after its
//! effect stack), **Corrector** nodes (each a full Lumetri instance: primaries, curves, wheels,
//! secondaries, LUTs), **Parallel** and **Layer** mixers that combine branches, and an **Output**
//! node. Serial correctors chain one after another; a mixer takes several branches that all start
//! from the same upstream picture. The graph is evaluated by the renderer
//! (`filmcraft_render::grade`); this module only holds the model and its editing operations.
//!
//! Every editing operation keeps the graph valid (no cycles, every link between existing nodes,
//! one link per mixer input) and bounded ([`MAX_NODES`]); a graph loaded from a file is checked
//! again by [`GradeGraph::order`] before it is rendered, so a corrupt file renders as a pass-through
//! instead of crashing.

use serde::{Deserialize, Serialize};

use crate::effect::{EffectInstance, find_effect};

/// Node ids are stable for the life of the graph (never reused).
pub type NodeId = u32;

/// The most nodes a graph may hold (a hostile file cannot make the evaluator allocate without bound).
pub const MAX_NODES: usize = 256;
/// The most inputs a mixer takes.
pub const MAX_MIXER_INPUTS: u32 = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GradeNodeKind {
    /// The clip's picture entering the graph.
    Input,
    /// The graded picture leaving the graph.
    Output,
    /// A full colour corrector (a Lumetri instance).
    Corrector { effect: EffectInstance },
    /// Averages its inputs: each branch contributes equally.
    Parallel,
    /// Composites input 2 (and up) over input 1 with a blend mode and opacity.
    Layer { blend: u32, opacity: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradeNode {
    pub id: NodeId,
    #[serde(flatten)]
    pub kind: GradeNodeKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Canvas position (node-editor points).
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

fn yes() -> bool {
    true
}

/// `from`'s output feeds `to`'s input number `input` (0-based; correctors and Output have one input).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradeLink {
    pub from: NodeId,
    pub to: NodeId,
    #[serde(default)]
    pub input: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GradeGraph {
    pub nodes: Vec<GradeNode>,
    pub links: Vec<GradeLink>,
    pub next_id: NodeId,
}

impl Default for GradeGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// What went wrong with an edit of the graph.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GradeError {
    #[error("no node {0}")]
    NoNode(NodeId),
    #[error("node {0} cannot be removed")]
    Fixed(NodeId),
    #[error("node {0} is not a mixer")]
    NotMixer(NodeId),
    #[error("input {0} is out of range")]
    BadInput(u32),
    #[error("that link would make a cycle")]
    Cycle,
    #[error("the graph is full ({MAX_NODES} nodes)")]
    Full,
    #[error("the graph is not valid")]
    Invalid,
}

pub const INPUT_ID: NodeId = 0;
pub const OUTPUT_ID: NodeId = 1;

const NODE_W: f32 = 104.0;
const NODE_GAP: f32 = 40.0;
const ROW_H: f32 = 72.0;

impl GradeGraph {
    /// Input → one corrector → Output.
    pub fn new() -> Self {
        let mut g = Self {
            nodes: vec![
                GradeNode { id: INPUT_ID, kind: GradeNodeKind::Input, label: String::new(), enabled: true, x: 0.0, y: 0.0 },
                GradeNode { id: OUTPUT_ID, kind: GradeNodeKind::Output, label: String::new(), enabled: true, x: 2.0 * (NODE_W + NODE_GAP), y: 0.0 },
            ],
            links: vec![GradeLink { from: INPUT_ID, to: OUTPUT_ID, input: 0 }],
            next_id: 2,
        };
        // a fresh graph has one corrector to start grading in
        let _ = g.add_serial_after(INPUT_ID);
        g
    }

    pub fn node(&self, id: NodeId) -> Option<&GradeNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut GradeNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
    /// The corrector instance of node `id`, if it is one.
    pub fn corrector(&self, id: NodeId) -> Option<&EffectInstance> {
        match &self.node(id)?.kind {
            GradeNodeKind::Corrector { effect } => Some(effect),
            _ => None,
        }
    }
    pub fn corrector_mut(&mut self, id: NodeId) -> Option<&mut EffectInstance> {
        match &mut self.node_mut(id)?.kind {
            GradeNodeKind::Corrector { effect } => Some(effect),
            _ => None,
        }
    }
    /// Corrector node ids in evaluation order (the "node numbers" shown in the editor).
    pub fn correctors(&self) -> Vec<NodeId> {
        self.order().unwrap_or_default().into_iter().filter(|id| self.corrector(*id).is_some()).collect()
    }
    /// The number a corrector node shows (1-based position among the correctors), if it is one.
    pub fn number_of(&self, id: NodeId) -> Option<usize> {
        self.correctors().iter().position(|n| *n == id).map(|p| p.saturating_add(1))
    }
    /// How many inputs a node takes.
    pub fn input_count(&self, id: NodeId) -> u32 {
        match self.node(id).map(|n| &n.kind) {
            Some(GradeNodeKind::Input) => 0,
            Some(GradeNodeKind::Parallel | GradeNodeKind::Layer { .. }) => {
                // the inputs in use plus one free slot, at least two
                let used = self.links.iter().filter(|l| l.to == id).map(|l| l.input.saturating_add(1)).max().unwrap_or(0);
                used.saturating_add(1).clamp(2, MAX_MIXER_INPUTS)
            }
            Some(_) => 1,
            None => 0,
        }
    }
    /// The links feeding `id`, by input number.
    pub fn inputs_of(&self, id: NodeId) -> Vec<GradeLink> {
        let mut v: Vec<GradeLink> = self.links.iter().copied().filter(|l| l.to == id).collect();
        v.sort_by_key(|l| l.input);
        v
    }
    /// The nodes `id` feeds.
    pub fn outputs_of(&self, id: NodeId) -> Vec<GradeLink> {
        self.links.iter().copied().filter(|l| l.from == id).collect()
    }
    /// True when the graph changes the picture: an enabled corrector or mixer lies on a path to Output.
    pub fn is_active(&self) -> bool {
        self.order()
            .is_ok_and(|o| o.iter().any(|id| self.node(*id).is_some_and(|n| n.enabled && !matches!(n.kind, GradeNodeKind::Input | GradeNodeKind::Output))))
    }
    /// The correctors of a graph that is one serial chain Input → c1 → c2 … → Output (no mixers),
    /// in order: such a grade runs as plain effects (on the GPU too). None for any other shape.
    pub fn serial_chain(&self) -> Option<Vec<&EffectInstance>> {
        let order = self.order().ok()?;
        let mut out = Vec::new();
        let mut cur = INPUT_ID;
        loop {
            let next: Vec<GradeLink> = self.outputs_of(cur);
            let [l] = next.as_slice() else { return None };
            if self.inputs_of(l.to).len() != 1 {
                return None;
            }
            match &self.node(l.to)?.kind {
                GradeNodeKind::Output => break,
                GradeNodeKind::Corrector { effect } => {
                    if self.node(l.to).is_some_and(|n| n.enabled) {
                        out.push(effect);
                    }
                }
                _ => return None,
            }
            cur = l.to;
            if out.len() > order.len() {
                return None;
            }
        }
        Some(out)
    }

    /// Nodes in an order where every node comes after its inputs (Kahn's algorithm), or
    /// [`GradeError::Invalid`] for a cyclic or otherwise inconsistent graph. Bounded by
    /// [`MAX_NODES`].
    pub fn order(&self) -> Result<Vec<NodeId>, GradeError> {
        if self.nodes.len() > MAX_NODES || self.links.len() > MAX_NODES * MAX_MIXER_INPUTS as usize {
            return Err(GradeError::Invalid);
        }
        let n = self.nodes.len();
        // links must join existing nodes; no two links into the same input
        let mut indeg = vec![0usize; n];
        for l in &self.links {
            let (Some(_), Some(ti)) = (self.index(l.from), self.index(l.to)) else { return Err(GradeError::Invalid) };
            if self.links.iter().filter(|o| o.to == l.to && o.input == l.input).count() != 1 {
                return Err(GradeError::Invalid);
            }
            if let Some(d) = indeg.get_mut(ti) {
                *d = d.saturating_add(1);
            }
        }
        let mut ready: Vec<usize> = (0..n).filter(|i| indeg.get(*i).copied() == Some(0)).collect();
        let mut out = Vec::with_capacity(n);
        let mut steps = 0usize;
        while let Some(i) = ready.pop() {
            steps = steps.saturating_add(1);
            if steps > n.saturating_add(1) {
                return Err(GradeError::Invalid);
            }
            let Some(node) = self.nodes.get(i) else { return Err(GradeError::Invalid) };
            out.push(node.id);
            for l in self.links.iter().filter(|l| l.from == node.id) {
                if let Some(ti) = self.index(l.to)
                    && let Some(d) = indeg.get_mut(ti)
                {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        ready.push(ti);
                    }
                }
            }
            // stable: lower ids first among the ready set
            ready.sort_by(|a, b| b.cmp(a));
        }
        if out.len() != n {
            return Err(GradeError::Invalid);
        }
        Ok(out)
    }

    fn index(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    fn fresh_corrector(&mut self) -> Result<NodeId, GradeError> {
        if self.nodes.len() >= MAX_NODES {
            return Err(GradeError::Full);
        }
        let effect = find_effect("lumetri").map(|d| d.instance()).ok_or(GradeError::Invalid)?;
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(GradeError::Full)?;
        self.nodes.push(GradeNode { id, kind: GradeNodeKind::Corrector { effect }, label: String::new(), enabled: true, x: 0.0, y: 0.0 });
        Ok(id)
    }

    fn fresh_mixer(&mut self, kind: GradeNodeKind) -> Result<NodeId, GradeError> {
        if self.nodes.len() >= MAX_NODES {
            return Err(GradeError::Full);
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(GradeError::Full)?;
        self.nodes.push(GradeNode { id, kind, label: String::new(), enabled: true, x: 0.0, y: 0.0 });
        Ok(id)
    }

    /// Insert a new corrector right after `after`, taking over everything `after` fed. Returns its id.
    pub fn add_serial_after(&mut self, after: NodeId) -> Result<NodeId, GradeError> {
        let after_node = self.node(after).ok_or(GradeError::NoNode(after))?;
        if matches!(after_node.kind, GradeNodeKind::Output) {
            return Err(GradeError::Fixed(after));
        }
        let (ax, ay) = (after_node.x, after_node.y);
        let id = self.fresh_corrector()?;
        for l in self.links.iter_mut().filter(|l| l.from == after) {
            l.from = id;
        }
        self.links.push(GradeLink { from: after, to: id, input: 0 });
        if let Some(n) = self.node_mut(id) {
            n.x = ax + NODE_W + NODE_GAP;
            n.y = ay;
        }
        self.shift_downstream(id, NODE_W + NODE_GAP);
        Ok(id)
    }

    /// Insert a new corrector right before `before`, fed by what fed `before`'s first input.
    pub fn add_serial_before(&mut self, before: NodeId) -> Result<NodeId, GradeError> {
        let node = self.node(before).ok_or(GradeError::NoNode(before))?;
        if matches!(node.kind, GradeNodeKind::Input) {
            return Err(GradeError::Fixed(before));
        }
        let (bx, by) = (node.x, node.y);
        let id = self.fresh_corrector()?;
        let mut had_input = false;
        for l in self.links.iter_mut().filter(|l| l.to == before && l.input == 0) {
            l.to = id;
            had_input = true;
        }
        let _ = had_input;
        self.links.push(GradeLink { from: id, to: before, input: 0 });
        if let Some(n) = self.node_mut(id) {
            n.x = bx;
            n.y = by;
        }
        self.shift_downstream(id, NODE_W + NODE_GAP);
        Ok(id)
    }

    /// Add a mixer after `at`: `at` keeps its downstream through the mixer's input 1, and a new
    /// corrector branch from `at`'s own input feeds input 2. Returns (mixer, new corrector).
    pub fn add_mixer_after(&mut self, at: NodeId, kind: GradeNodeKind) -> Result<(NodeId, NodeId), GradeError> {
        if !matches!(kind, GradeNodeKind::Parallel | GradeNodeKind::Layer { .. }) {
            return Err(GradeError::Invalid);
        }
        let node = self.node(at).ok_or(GradeError::NoNode(at))?;
        if matches!(node.kind, GradeNodeKind::Output) {
            return Err(GradeError::Fixed(at));
        }
        let (ax, ay) = (node.x, node.y);
        if self.nodes.len().saturating_add(2) > MAX_NODES {
            return Err(GradeError::Full);
        }
        // the branch starts where `at` starts (its input), or at `at` itself when it is the Input node
        let source = self.inputs_of(at).first().map(|l| l.from).unwrap_or(at);
        let mixer = self.fresh_mixer(kind)?;
        let branch = self.fresh_corrector()?;
        for l in self.links.iter_mut().filter(|l| l.from == at) {
            l.from = mixer;
        }
        self.links.push(GradeLink { from: at, to: mixer, input: 0 });
        self.links.push(GradeLink { from: source, to: branch, input: 0 });
        self.links.push(GradeLink { from: branch, to: mixer, input: 1 });
        if let Some(n) = self.node_mut(mixer) {
            n.x = ax + NODE_W + NODE_GAP;
            n.y = ay;
        }
        if let Some(n) = self.node_mut(branch) {
            n.x = ax;
            n.y = ay + ROW_H;
        }
        self.shift_downstream(mixer, NODE_W + NODE_GAP);
        Ok((mixer, branch))
    }

    /// Add a new corrector branch into a free input of mixer `mixer`, fed from the mixer's first
    /// branch's source. Returns the new corrector.
    pub fn add_branch(&mut self, mixer: NodeId) -> Result<NodeId, GradeError> {
        let node = self.node(mixer).ok_or(GradeError::NoNode(mixer))?;
        if !matches!(node.kind, GradeNodeKind::Parallel | GradeNodeKind::Layer { .. }) {
            return Err(GradeError::NotMixer(mixer));
        }
        let (mx, my) = (node.x, node.y);
        let inputs = self.inputs_of(mixer);
        let used: Vec<u32> = inputs.iter().map(|l| l.input).collect();
        let slot = (0..MAX_MIXER_INPUTS).find(|i| !used.contains(i)).ok_or(GradeError::BadInput(MAX_MIXER_INPUTS))?;
        let source = inputs.first().and_then(|l| self.inputs_of(l.from).first().map(|u| u.from)).unwrap_or(INPUT_ID);
        let branch = self.fresh_corrector()?;
        self.links.push(GradeLink { from: source, to: branch, input: 0 });
        self.links.push(GradeLink { from: branch, to: mixer, input: slot });
        if let Some(n) = self.node_mut(branch) {
            n.x = mx - NODE_W - NODE_GAP;
            n.y = my + ROW_H * slot as f32;
        }
        Ok(branch)
    }

    /// Remove a node. A node with one input and outputs hands its input on to them (a serial
    /// corrector drops out of its chain); Input and Output cannot be removed.
    pub fn remove(&mut self, id: NodeId) -> Result<(), GradeError> {
        let node = self.node(id).ok_or(GradeError::NoNode(id))?;
        if matches!(node.kind, GradeNodeKind::Input | GradeNodeKind::Output) {
            return Err(GradeError::Fixed(id));
        }
        let upstream = self.inputs_of(id).first().map(|l| l.from);
        let outs = self.outputs_of(id);
        self.links.retain(|l| l.from != id && l.to != id);
        if let Some(up) = upstream {
            for o in outs {
                if !self.links.iter().any(|l| l.to == o.to && l.input == o.input) {
                    self.links.push(GradeLink { from: up, to: o.to, input: o.input });
                }
            }
        }
        self.nodes.retain(|n| n.id != id);
        // never leave Output unfed: a dangling Output passes the Input through
        if self.inputs_of(OUTPUT_ID).is_empty() {
            self.links.push(GradeLink { from: INPUT_ID, to: OUTPUT_ID, input: 0 });
        }
        if self.order().is_err() {
            return Err(GradeError::Invalid);
        }
        Ok(())
    }

    /// Connect `from` → `to` at `input`, replacing what fed that input. Refused when it would
    /// make a cycle or address an input the node does not have.
    pub fn link(&mut self, from: NodeId, to: NodeId, input: u32) -> Result<(), GradeError> {
        self.node(from).ok_or(GradeError::NoNode(from))?;
        let to_node = self.node(to).ok_or(GradeError::NoNode(to))?;
        if from == to || matches!(to_node.kind, GradeNodeKind::Input) || matches!(self.node(from).map(|n| &n.kind), Some(GradeNodeKind::Output)) {
            return Err(GradeError::Cycle);
        }
        let max_in = match to_node.kind {
            GradeNodeKind::Parallel | GradeNodeKind::Layer { .. } => MAX_MIXER_INPUTS,
            _ => 1,
        };
        if input >= max_in {
            return Err(GradeError::BadInput(input));
        }
        let before = self.links.clone();
        self.links.retain(|l| !(l.to == to && l.input == input));
        self.links.push(GradeLink { from, to, input });
        if self.order().is_err() {
            self.links = before;
            return Err(GradeError::Cycle);
        }
        Ok(())
    }

    /// Disconnect whatever feeds `to`'s `input`.
    pub fn unlink(&mut self, to: NodeId, input: u32) -> Result<(), GradeError> {
        self.node(to).ok_or(GradeError::NoNode(to))?;
        let n = self.links.len();
        self.links.retain(|l| !(l.to == to && l.input == input));
        if self.links.len() == n {
            return Err(GradeError::BadInput(input));
        }
        Ok(())
    }

    /// Move every node downstream of `from` right by `dx` so an inserted node has room.
    fn shift_downstream(&mut self, from: NodeId, dx: f32) {
        let Ok(order) = self.order() else { return };
        let Some(fx) = self.node(from).map(|n| n.x) else { return };
        let mut reach = vec![from];
        let mut i = 0;
        while let Some(id) = reach.get(i).copied() {
            i += 1;
            for l in self.outputs_of(id) {
                if !reach.contains(&l.to) && reach.len() <= order.len() {
                    reach.push(l.to);
                }
            }
        }
        for n in self.nodes.iter_mut().filter(|n| n.id != from && reach.contains(&n.id) && n.x >= fx) {
            n.x += dx;
        }
    }

    /// Lay the nodes out again from their links: columns by depth, rows by branch.
    pub fn auto_layout(&mut self) {
        let Ok(order) = self.order() else { return };
        let mut depth: Vec<(NodeId, usize)> = Vec::with_capacity(order.len());
        for id in &order {
            let d = self.inputs_of(*id).iter().filter_map(|l| depth.iter().find(|(n, _)| *n == l.from).map(|(_, d)| d.saturating_add(1))).max().unwrap_or(0);
            depth.push((*id, d));
        }
        let mut rows: Vec<usize> = Vec::new();
        for (id, d) in &depth {
            let row = rows.iter().filter(|r| **r == *d).count();
            rows.push(*d);
            if let Some(n) = self.node_mut(*id) {
                n.x = *d as f32 * (NODE_W + NODE_GAP);
                n.y = row as f32 * ROW_H;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_graph_is_input_corrector_output() {
        let g = GradeGraph::new();
        assert_eq!(g.correctors().len(), 1);
        let chain = g.serial_chain().unwrap();
        assert_eq!(chain.len(), 1);
        assert!(!g.is_active() || chain[0].enabled);
        assert_eq!(g.number_of(2), Some(1));
    }

    #[test]
    fn serial_insert_splices_the_chain() {
        let mut g = GradeGraph::new();
        let c2 = g.add_serial_after(2).unwrap();
        let c3 = g.add_serial_before(OUTPUT_ID).unwrap();
        assert_eq!(g.correctors(), vec![2, c2, c3]);
        assert_eq!(g.serial_chain().unwrap().len(), 3);
        g.remove(c2).unwrap();
        assert_eq!(g.correctors(), vec![2, c3]);
        assert_eq!(g.inputs_of(c3)[0].from, 2);
    }

    #[test]
    fn mixers_break_the_serial_chain_and_keep_the_output_fed() {
        let mut g = GradeGraph::new();
        let (mixer, branch) = g.add_mixer_after(2, GradeNodeKind::Parallel).unwrap();
        assert!(g.serial_chain().is_none());
        assert_eq!(g.inputs_of(mixer).len(), 2);
        assert_eq!(g.inputs_of(branch)[0].from, INPUT_ID);
        assert_eq!(g.inputs_of(OUTPUT_ID)[0].from, mixer);
        let b2 = g.add_branch(mixer).unwrap();
        assert_eq!(g.inputs_of(mixer).len(), 3);
        assert_eq!(g.input_count(mixer), 4);
        g.remove(mixer).unwrap();
        assert!(g.order().is_ok());
        assert!(!g.inputs_of(OUTPUT_ID).is_empty());
        let _ = b2;
    }

    #[test]
    fn cycles_and_bad_links_are_refused() {
        let mut g = GradeGraph::new();
        let c2 = g.add_serial_after(2).unwrap();
        assert_eq!(g.link(c2, 2, 0), Err(GradeError::Cycle));
        assert_eq!(g.link(2, INPUT_ID, 0), Err(GradeError::Cycle));
        assert_eq!(g.link(2, c2, 5), Err(GradeError::BadInput(5)));
        assert_eq!(g.link(99, c2, 0), Err(GradeError::NoNode(99)));
        assert_eq!(g.remove(INPUT_ID), Err(GradeError::Fixed(INPUT_ID)));
        assert!(g.order().is_ok());
    }

    #[test]
    fn corrupt_graphs_fail_order_instead_of_looping() {
        let mut g = GradeGraph::new();
        g.links.push(GradeLink { from: OUTPUT_ID, to: INPUT_ID, input: 0 });
        assert_eq!(g.order(), Err(GradeError::Invalid));
        let mut g = GradeGraph::new();
        g.links.push(GradeLink { from: 77, to: OUTPUT_ID, input: 3 });
        assert_eq!(g.order(), Err(GradeError::Invalid));
        assert!(g.serial_chain().is_none());
        assert!(!g.is_active());
    }

    #[test]
    fn roundtrips_through_json() {
        let mut g = GradeGraph::new();
        let (_, _) = g.add_mixer_after(2, GradeNodeKind::Layer { blend: 3, opacity: 50.0 }).unwrap();
        let s = serde_json::to_string(&g).unwrap();
        let back: GradeGraph = serde_json::from_str(&s).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn auto_layout_places_columns_by_depth() {
        let mut g = GradeGraph::new();
        let (mixer, _) = g.add_mixer_after(2, GradeNodeKind::Parallel).unwrap();
        g.auto_layout();
        assert!(g.node(mixer).unwrap().x > g.node(2).unwrap().x);
        assert!(g.node(OUTPUT_ID).unwrap().x > g.node(mixer).unwrap().x);
    }
}
