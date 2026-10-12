//! Evaluation of a clip's node grade ([`filmcraft_project::GradeGraph`]) on the CPU reference path.
//!
//! Nodes run in dependency order ([`GradeGraph::order`]); each keeps its result until every node it
//! feeds has consumed it. A corrector applies its Lumetri instance through [`effects::apply`], so a
//! node grades exactly as the same instance would in the effect stack. A disabled node, a node whose
//! input is missing, and a graph that fails validation all pass their input through unchanged: a bad
//! file never blanks a monitor.

use std::collections::HashMap;

use filmcraft_project::{GradeGraph, GradeNodeKind, INPUT_ID, NodeId, OUTPUT_ID};

use crate::blend::{Blend, composite};
use crate::effects::{self, FxCtx};
use crate::image::Image;

/// Grade `img` in place.
pub fn apply(img: &mut Image, graph: &GradeGraph, cx: &FxCtx) {
    let Ok(order) = graph.order() else { return };
    // how many consumers each node's result still has
    let mut pending: HashMap<NodeId, usize> = HashMap::with_capacity(order.len());
    for id in &order {
        pending.insert(*id, graph.outputs_of(*id).len());
    }
    let mut results: HashMap<NodeId, Image> = HashMap::with_capacity(order.len());
    for id in &order {
        let Some(node) = graph.node(*id) else { continue };
        let inputs = graph.inputs_of(*id);
        // the picture on a node's first input (the one a bypassed node passes through)
        let take = |results: &mut HashMap<NodeId, Image>, pending: &mut HashMap<NodeId, usize>, from: NodeId| -> Option<Image> {
            let left = pending.get_mut(&from)?;
            *left = left.saturating_sub(1);
            if *left == 0 { results.remove(&from) } else { results.get(&from).cloned() }
        };
        let out = match &node.kind {
            GradeNodeKind::Input => Some(img.clone()),
            GradeNodeKind::Output | GradeNodeKind::Corrector { .. } => {
                let mut cur = inputs.first().and_then(|l| take(&mut results, &mut pending, l.from));
                if let (GradeNodeKind::Corrector { effect }, Some(cur), true) = (&node.kind, cur.as_mut(), node.enabled)
                    && effect.enabled
                {
                    effects::apply(cur, effect, cx);
                }
                cur
            }
            GradeNodeKind::Parallel => {
                let mut branches: Vec<Image> = inputs.iter().filter_map(|l| take(&mut results, &mut pending, l.from)).collect();
                if !node.enabled || branches.len() <= 1 {
                    branches.into_iter().next()
                } else {
                    let n = branches.len() as f32;
                    let mut acc = branches.remove(0);
                    let same = branches.iter().all(|b| b.w == acc.w && b.h == acc.h && b.px.len() == acc.px.len());
                    if same {
                        for b in &branches {
                            for (a, v) in acc.px.iter_mut().zip(b.px.iter()) {
                                *a += *v;
                            }
                        }
                        for a in acc.px.iter_mut() {
                            *a /= n;
                        }
                    }
                    Some(acc)
                }
            }
            GradeNodeKind::Layer { blend, opacity } => {
                let mut branches: Vec<Image> = inputs.iter().filter_map(|l| take(&mut results, &mut pending, l.from)).collect();
                if !node.enabled || branches.len() <= 1 {
                    branches.into_iter().next()
                } else {
                    let mut base = branches.remove(0);
                    let op = if opacity.is_finite() { (*opacity / 100.0).clamp(0.0, 1.0) as f32 } else { 1.0 };
                    let mode = Blend::from_index(*blend);
                    for top in &branches {
                        if top.w == base.w && top.h == base.h {
                            composite(&mut base, top, op, mode);
                        }
                    }
                    Some(base)
                }
            }
        };
        if let Some(out) = out {
            if *id == OUTPUT_ID {
                *img = out;
                return;
            }
            if pending.get(id).copied().unwrap_or(0) > 0 {
                results.insert(*id, out);
            }
        }
    }
    // Output never produced a picture (unfed): leave the input as it is
    let _ = INPUT_ID;
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{GradeGraph, GradeNodeKind, ParamValue};
    use filmcraft_time::Tick;

    fn cx() -> FxCtx<'static> {
        FxCtx {
            t: Tick(0),
            px_scale: 1.0,
            seconds: 0.0,
            timecode: "",
            clip_name: "",
            project: None,
            env: None,
            working: filmcraft_color::WorkingSpace::default(),
        }
    }

    fn gradient() -> Image {
        let mut img = Image::new(8, 4);
        for y in 0..4 {
            for x in 0..8 {
                let v = x as f32 / 7.0;
                let i = (y * 8 + x) * 4;
                img.px[i..i + 4].copy_from_slice(&[v, v * 0.5, 0.2, 1.0]);
            }
        }
        img
    }

    fn set(g: &mut GradeGraph, node: NodeId, param: &str, v: f64) {
        let e = g.corrector_mut(node).unwrap();
        e.params.get_mut(param).unwrap().value = ParamValue::Float(v);
    }

    fn close(a: &Image, b: &Image) -> bool {
        a.w == b.w && a.h == b.h && a.px.iter().zip(&b.px).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn a_serial_node_grades_like_the_effect() {
        let mut g = GradeGraph::new();
        set(&mut g, 2, "exposure", 1.0);
        let mut via_graph = gradient();
        apply(&mut via_graph, &g, &cx());
        let mut direct = gradient();
        effects::apply(&mut direct, g.corrector(2).unwrap(), &cx());
        assert!(close(&via_graph, &direct));
        assert!(!close(&via_graph, &gradient()), "exposure changes the picture");
    }

    #[test]
    fn disabled_nodes_and_bad_graphs_pass_through() {
        let mut g = GradeGraph::new();
        set(&mut g, 2, "exposure", 2.0);
        g.node_mut(2).unwrap().enabled = false;
        let mut img = gradient();
        apply(&mut img, &g, &cx());
        assert!(close(&img, &gradient()));
        g.node_mut(2).unwrap().enabled = true;
        g.links.push(filmcraft_project::GradeLink { from: OUTPUT_ID, to: 2, input: 0 });
        let mut img = gradient();
        apply(&mut img, &g, &cx());
        assert!(close(&img, &gradient()));
    }

    #[test]
    fn layer_mixer_at_full_opacity_shows_the_top_branch_and_parallel_averages() {
        let mut g = GradeGraph::new();
        let (mixer, branch) = g.add_mixer_after(2, GradeNodeKind::Layer { blend: 0, opacity: 100.0 }).unwrap();
        set(&mut g, branch, "exposure", 1.0);
        let mut layered = gradient();
        apply(&mut layered, &g, &cx());
        let mut top_only = gradient();
        effects::apply(&mut top_only, g.corrector(branch).unwrap(), &cx());
        assert!(close(&layered, &top_only));
        // half opacity sits between the branches
        if let Some(GradeNodeKind::Layer { opacity, .. }) = g.node_mut(mixer).map(|n| &mut n.kind) {
            *opacity = 50.0;
        }
        let mut half = gradient();
        apply(&mut half, &g, &cx());
        let p = half.get(7, 0)[0];
        assert!(p > gradient().get(7, 0)[0] - 1e-4 && p < top_only.get(7, 0)[0] + 1e-4);
        // a parallel mixer averages
        let mut g = GradeGraph::new();
        let (_, branch) = g.add_mixer_after(2, GradeNodeKind::Parallel).unwrap();
        set(&mut g, branch, "exposure", 1.0);
        let mut avg = gradient();
        apply(&mut avg, &g, &cx());
        let expect = (gradient().get(7, 0)[0] + top_only.get(7, 0)[0]) / 2.0;
        assert!((avg.get(7, 0)[0] - expect).abs() < 1e-4);
    }
}
