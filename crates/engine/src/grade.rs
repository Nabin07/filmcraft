//! `grade.*`: the Color page's node graph ([`filmcraft_project::GradeGraph`]) on a clip.
//!
//! Every command takes `clip` (default: the selected video clip, as the Lumetri commands do) and
//! addresses nodes by id. `grade.addSerial`, `grade.addParallel` and `grade.addLayer` start a graph
//! on a clip that has none. A corrector node's parameters are set through `effects.setParam` with
//! `node` in place of `effect`; `lumetri.setInputLut`, `lumetri.setLook`, `lumetri.setSection` and
//! `lumetri.applyMatch` take `node` the same way.

use filmcraft_project::{ClipId, GradeGraph, GradeNodeKind, NodeId, TrackItem, TrackKind};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, clip_p, f64_p, has_seq, str_p, u64_p};
use crate::{Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;

fn spec(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: has_seq, run, journal: true }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "grade.info",
            label: "Node Grade",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id?}"#,
            enabled: has_seq,
            run: info,
            journal: false,
        },
        spec("grade.start", "Start Node Grade", r#"{"clip":id?}"#, start),
        spec("grade.addSerial", "Add Serial Node", r#"{"clip":id?,"after":node?|"before":node?}"#, add_serial),
        spec("grade.addParallel", "Add Parallel Node", r#"{"clip":id?,"at":node?}"#, |s, p| add_mixer(s, p, GradeNodeKind::Parallel, "grade.addParallel")),
        spec("grade.addLayer", "Add Layer Node", r#"{"clip":id?,"at":node?}"#, |s, p| {
            add_mixer(s, p, GradeNodeKind::Layer { blend: 0, opacity: 100.0 }, "grade.addLayer")
        }),
        spec("grade.addBranch", "Add Branch", r#"{"clip":id?,"mixer":node}"#, add_branch),
        spec("grade.removeNode", "Delete Node", r#"{"clip":id?,"node":node}"#, remove_node),
        spec("grade.link", "Connect Nodes", r#"{"clip":id?,"from":node,"to":node,"input":n=0}"#, link),
        spec("grade.unlink", "Disconnect Input", r#"{"clip":id?,"to":node,"input":n=0}"#, unlink),
        spec("grade.setEnabled", "Enable / Disable Node", r#"{"clip":id?,"node":node,"enabled":bool? (default: toggle)}"#, set_enabled),
        spec(
            "grade.moveNode",
            "Move Node",
            r#"{"clip":id?,"node":node,"x":num,"y":num,"begin":bool? (start a new undo step; a drag merges into one)}"#,
            move_node,
        ),
        spec("grade.setLabel", "Label Node", r#"{"clip":id?,"node":node,"label":str}"#, set_label),
        spec("grade.setMixer", "Set Mixer", r#"{"clip":id?,"node":node,"blend":index|name?,"opacity":0..100?}"#, set_mixer),
        spec("grade.resetNode", "Reset Node", r#"{"clip":id?,"node":node}"#, reset_node),
        spec("grade.autoLayout", "Auto Layout Nodes", r#"{"clip":id?}"#, auto_layout),
        spec("grade.reset", "Remove Node Grade", r#"{"clip":id?}"#, reset),
    ]
}

/// The clip a grade command works on: `clip`, else the selected video clip.
pub(crate) fn grade_clip(s: &Session, p: &Value, cmd: &str) -> Result<ClipId> {
    let seq = s.active_sequence().ok_or_else(|| bad(cmd, "no active sequence"))?;
    let video = |c: &ClipId| seq.find_item(*c).is_some_and(|(t, _)| seq.track(t).is_some_and(|t| t.kind == TrackKind::Video));
    match clip_p(p, "clip") {
        Some(c) if video(&c) => Ok(c),
        Some(_) => Err(bad(cmd, "not a video clip")),
        None => {
            let has = |c: &ClipId| seq.find_item(*c).is_some_and(|(_, it)| it.grade.is_some());
            let sel: Vec<ClipId> = s.state.selection.iter().copied().filter(video).collect();
            sel.iter().copied().find(has).or(sel.first().copied()).ok_or_else(|| bad(cmd, "select a video clip"))
        }
    }
}

fn node_p(p: &Value, k: &str) -> Option<NodeId> {
    u64_p(p, k).and_then(|n| u32::try_from(n).ok())
}

/// The clip's graph, started when it has none.
fn graph_mut(it: &mut TrackItem) -> &mut GradeGraph {
    it.grade.get_or_insert_with(GradeGraph::new)
}

fn graph_err(cmd: &str, e: filmcraft_project::GradeError) -> crate::EngineError {
    bad(cmd, e.to_string())
}

/// Run `f` on the clip's graph (created when `create`), as one undo step.
fn edit<R>(
    s: &mut Session,
    p: &Value,
    cmd: &str,
    label: &str,
    create: bool,
    merge: Option<&str>,
    f: impl FnOnce(&mut GradeGraph) -> Result<R>,
) -> Result<(ClipId, R)> {
    let clip = grade_clip(s, p, cmd)?;
    let r = s.edit_sequence_as(label, merge, |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        if !create && it.grade.is_none() {
            return Err(bad(cmd, "the clip has no node grade"));
        }
        f(graph_mut(it))
    })?;
    Ok((clip, r))
}

pub(crate) fn graph_json(g: &GradeGraph) -> Value {
    let order = g.order().unwrap_or_default();
    json!({
        "valid": g.order().is_ok(),
        "active": g.is_active(),
        "serial": g.serial_chain().is_some(),
        "order": order,
        "nodes": g.nodes.iter().map(|n| {
            let kind = match &n.kind {
                GradeNodeKind::Input => "input",
                GradeNodeKind::Output => "output",
                GradeNodeKind::Corrector { .. } => "corrector",
                GradeNodeKind::Parallel => "parallel",
                GradeNodeKind::Layer { .. } => "layer",
            };
            let mut v = json!({"id": n.id, "kind": kind, "label": n.label, "enabled": n.enabled, "x": n.x, "y": n.y, "inputs": g.input_count(n.id), "number": g.number_of(n.id)});
            if let GradeNodeKind::Layer { blend, opacity } = &n.kind {
                v["blend"] = json!(filmcraft_project::effect::BLEND_MODES.get(*blend as usize).copied().unwrap_or("Normal"));
                v["opacity"] = json!(opacity);
            }
            v
        }).collect::<Vec<_>>(),
        "links": g.links.iter().map(|l| json!({"from": l.from, "to": l.to, "input": l.input})).collect::<Vec<_>>(),
    })
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = grade_clip(s, p, "grade.info")?;
    let seq = s.active_sequence().ok_or_else(|| bad("grade.info", "no active sequence"))?;
    let (_, it) = seq.find_item(clip).ok_or_else(|| bad("grade.info", "no such clip"))?;
    Ok(match &it.grade {
        Some(g) => {
            let mut v = graph_json(g);
            v["clip"] = json!(clip.0);
            v
        }
        None => json!({"clip": clip.0, "nodes": [], "links": [], "valid": true, "active": false}),
    })
}

/// Give the clip a graph (Input → one corrector → Output) when it has none; returns its first corrector.
fn start(s: &mut Session, p: &Value) -> Result<Value> {
    let (clip, node) = edit(s, p, "grade.start", "Start Node Grade", true, None, |g| Ok(g.correctors().first().copied()))?;
    Ok(json!({"clip": clip.0, "node": node}))
}

fn add_serial(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.addSerial";
    let (after, before) = (node_p(p, "after"), node_p(p, "before"));
    let (clip, node) = edit(s, p, cmd, "Add Serial Node", true, None, |g| {
        let r = match (after, before) {
            (_, Some(b)) => g.add_serial_before(b),
            (Some(a), None) => g.add_serial_after(a),
            // the last corrector, else right after Input
            (None, None) => g.add_serial_after(g.correctors().last().copied().unwrap_or(filmcraft_project::INPUT_ID)),
        };
        r.map_err(|e| graph_err(cmd, e))
    })?;
    Ok(json!({"clip": clip.0, "node": node}))
}

fn add_mixer(s: &mut Session, p: &Value, kind: GradeNodeKind, cmd: &'static str) -> Result<Value> {
    let at = node_p(p, "at");
    let (clip, (mixer, node)) =
        edit(s, p, cmd, if matches!(kind, GradeNodeKind::Parallel) { "Add Parallel Node" } else { "Add Layer Node" }, true, None, |g| {
            let at = at.unwrap_or_else(|| g.correctors().last().copied().unwrap_or(filmcraft_project::INPUT_ID));
            g.add_mixer_after(at, kind).map_err(|e| graph_err(cmd, e))
        })?;
    Ok(json!({"clip": clip.0, "mixer": mixer, "node": node}))
}

fn add_branch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.addBranch";
    let mixer = node_p(p, "mixer").ok_or_else(|| bad(cmd, "need `mixer`"))?;
    let (clip, node) = edit(s, p, cmd, "Add Branch", false, None, |g| g.add_branch(mixer).map_err(|e| graph_err(cmd, e)))?;
    Ok(json!({"clip": clip.0, "node": node}))
}

fn remove_node(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.removeNode";
    let node = node_p(p, "node").ok_or_else(|| bad(cmd, "need `node`"))?;
    let (clip, ()) = edit(s, p, cmd, "Delete Node", false, None, |g| g.remove(node).map_err(|e| graph_err(cmd, e)))?;
    Ok(json!({"clip": clip.0}))
}

fn link(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.link";
    let from = node_p(p, "from").ok_or_else(|| bad(cmd, "need `from`"))?;
    let to = node_p(p, "to").ok_or_else(|| bad(cmd, "need `to`"))?;
    let input = u64_p(p, "input").and_then(|i| u32::try_from(i).ok()).unwrap_or(0);
    let (clip, ()) = edit(s, p, cmd, "Connect Nodes", false, None, |g| g.link(from, to, input).map_err(|e| graph_err(cmd, e)))?;
    Ok(json!({"clip": clip.0}))
}

fn unlink(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.unlink";
    let to = node_p(p, "to").ok_or_else(|| bad(cmd, "need `to`"))?;
    let input = u64_p(p, "input").and_then(|i| u32::try_from(i).ok()).unwrap_or(0);
    let (clip, ()) = edit(s, p, cmd, "Disconnect Input", false, None, |g| g.unlink(to, input).map_err(|e| graph_err(cmd, e)))?;
    Ok(json!({"clip": clip.0}))
}

fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.setEnabled";
    let node = node_p(p, "node").ok_or_else(|| bad(cmd, "need `node`"))?;
    let want = bool_p(p, "enabled");
    let (clip, on) = edit(s, p, cmd, "Toggle Node", false, None, |g| {
        let n = g.node_mut(node).ok_or_else(|| graph_err(cmd, filmcraft_project::GradeError::NoNode(node)))?;
        n.enabled = want.unwrap_or(!n.enabled);
        Ok(n.enabled)
    })?;
    Ok(json!({"clip": clip.0, "node": node, "enabled": on}))
}

fn move_node(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.moveNode";
    let node = node_p(p, "node").ok_or_else(|| bad(cmd, "need `node`"))?;
    let coord = |k: &str| f64_p(p, k).filter(|v| v.is_finite()).map(|v| v.clamp(-100_000.0, 100_000.0) as f32);
    let (x, y) = (coord("x").ok_or_else(|| bad(cmd, "need finite `x`"))?, coord("y").ok_or_else(|| bad(cmd, "need finite `y`"))?);
    let clip = grade_clip(s, p, cmd)?;
    // a drag is one undo step; `begin` starts a new one
    let key = format!("grade.move:{}:{node}", clip.0);
    if bool_p(p, "begin").unwrap_or(false) {
        s.history.merge_key = None;
    }
    let (clip, ()) = edit(s, p, cmd, "Move Node", false, Some(&key), |g| {
        let n = g.node_mut(node).ok_or_else(|| graph_err(cmd, filmcraft_project::GradeError::NoNode(node)))?;
        n.x = x;
        n.y = y;
        Ok(())
    })?;
    Ok(json!({"clip": clip.0}))
}

fn set_label(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.setLabel";
    let node = node_p(p, "node").ok_or_else(|| bad(cmd, "need `node`"))?;
    let label: String = str_p(p, "label").unwrap_or("").chars().take(64).collect();
    let (clip, ()) = edit(s, p, cmd, "Label Node", false, None, |g| {
        let n = g.node_mut(node).ok_or_else(|| graph_err(cmd, filmcraft_project::GradeError::NoNode(node)))?;
        n.label = label;
        Ok(())
    })?;
    Ok(json!({"clip": clip.0}))
}

fn set_mixer(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.setMixer";
    let node = node_p(p, "node").ok_or_else(|| bad(cmd, "need `node`"))?;
    let blend = match p.get("blend") {
        Some(Value::Number(n)) => Some(
            n.as_u64()
                .and_then(|i| u32::try_from(i).ok())
                .filter(|i| (*i as usize) < filmcraft_project::effect::BLEND_MODES.len())
                .ok_or_else(|| bad(cmd, "unknown blend mode"))?,
        ),
        Some(Value::String(name)) => Some(
            filmcraft_project::effect::BLEND_MODES
                .iter()
                .position(|m| m.eq_ignore_ascii_case(name))
                .map(|i| i as u32)
                .ok_or_else(|| bad(cmd, format!("unknown blend mode `{name}`")))?,
        ),
        _ => None,
    };
    let opacity = f64_p(p, "opacity").map(|o| if o.is_finite() { o.clamp(0.0, 100.0) } else { 100.0 });
    let (clip, ()) = edit(s, p, cmd, "Set Mixer", false, None, |g| {
        let n = g.node_mut(node).ok_or_else(|| graph_err(cmd, filmcraft_project::GradeError::NoNode(node)))?;
        let GradeNodeKind::Layer { blend: b, opacity: o } = &mut n.kind else {
            return Err(graph_err(cmd, filmcraft_project::GradeError::NotMixer(node)));
        };
        if let Some(blend) = blend {
            *b = blend;
        }
        if let Some(opacity) = opacity {
            *o = opacity;
        }
        Ok(())
    })?;
    Ok(json!({"clip": clip.0}))
}

fn reset_node(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.resetNode";
    let node = node_p(p, "node").ok_or_else(|| bad(cmd, "need `node`"))?;
    let (clip, ()) = edit(s, p, cmd, "Reset Node", false, None, |g| {
        let e = g.corrector_mut(node).ok_or_else(|| graph_err(cmd, filmcraft_project::GradeError::NoNode(node)))?;
        *e = filmcraft_project::find_effect("lumetri").map(|d| d.instance()).ok_or_else(|| bad(cmd, "no Lumetri"))?;
        Ok(())
    })?;
    Ok(json!({"clip": clip.0}))
}

fn auto_layout(s: &mut Session, p: &Value) -> Result<Value> {
    let (clip, ()) = edit(s, p, "grade.autoLayout", "Auto Layout Nodes", false, None, |g| {
        g.auto_layout();
        Ok(())
    })?;
    Ok(json!({"clip": clip.0}))
}

fn reset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "grade.reset";
    let clip = grade_clip(s, p, cmd)?;
    s.edit_sequence("Remove Node Grade", |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        it.grade = None;
        Ok(())
    })?;
    Ok(json!({"clip": clip.0}))
}
