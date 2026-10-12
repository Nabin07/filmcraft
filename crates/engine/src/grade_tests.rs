use serde_json::json;

use crate::Session;
use filmcraft_project::ClipId;
use filmcraft_time::Tick;

fn demo() -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let it = q.video_tracks[0].items[0].clone();
    s.set_playhead(it.start + Tick(it.duration.0 / 2));
    s.state.selection = vec![it.id];
    (s, it.id)
}

fn luma(img: &filmcraft_render::Image) -> f32 {
    let n = (img.w * img.h).max(1) as f32;
    img.px.chunks(4).map(|p| p[0] + p[1] + p[2]).sum::<f32>() / n
}

#[test]
fn a_serial_node_grades_the_selected_clip_and_undoes() {
    let (mut s, clip) = demo();
    let before = luma(&s.render_program(0.2).unwrap());
    let r = s.execute("grade.addSerial", json!({})).unwrap();
    assert_eq!(r["clip"], clip.0);
    let node = r["node"].as_u64().unwrap();
    // the graph starts with one corrector (node 2); the new one follows it
    let info = s.execute("grade.info", json!({})).unwrap();
    assert_eq!(info["nodes"].as_array().unwrap().len(), 4);
    assert_eq!(info["serial"], true);
    assert_eq!(info["active"], true);
    assert!((luma(&s.render_program(0.2).unwrap()) - before).abs() < 1e-3, "default correctors leave the picture alone");
    s.execute("effects.setParam", json!({"clip": clip.0, "node": node, "param": "exposure", "value": 2.0})).unwrap();
    let after = luma(&s.render_program(0.2).unwrap());
    assert!(after > before + 0.05, "exposure on a node brightens the picture: {before} -> {after}");
    // bypassing the node restores the picture
    s.execute("grade.setEnabled", json!({"node": node})).unwrap();
    assert!((luma(&s.render_program(0.2).unwrap()) - before).abs() < 1e-3);
    s.undo();
    assert!(luma(&s.render_program(0.2).unwrap()) > before + 0.05);
    s.execute("grade.reset", json!({})).unwrap();
    assert!((luma(&s.render_program(0.2).unwrap()) - before).abs() < 1e-3);
}

#[test]
fn lumetri_commands_take_a_node() {
    let (mut s, clip) = demo();
    let node = s.execute("grade.addSerial", json!({})).unwrap()["node"].as_u64().unwrap();
    let r = s.execute("lumetri.setSection", json!({"node": node, "section": "vignette", "on": false})).unwrap();
    assert_eq!(r["on"], false);
    let q = s.active_sequence().unwrap();
    let (_, it) = q.find_item(clip).unwrap();
    let e = it.grade.as_ref().unwrap().corrector(node as u32).unwrap();
    assert_eq!(e.param("vignette_on").unwrap().value.as_bool(), Some(false));
    // the stack is untouched: no Lumetri was added to it
    assert!(!it.effects.iter().any(|e| e.effect == "lumetri"));
    assert!(s.execute("lumetri.setSection", json!({"node": 999, "section": "vignette"})).is_err());
}

#[test]
fn mixers_and_links() {
    let (mut s, _) = demo();
    let r = s.execute("grade.addLayer", json!({})).unwrap();
    let (mixer, branch) = (r["mixer"].as_u64().unwrap(), r["node"].as_u64().unwrap());
    let info = s.execute("grade.info", json!({})).unwrap();
    assert_eq!(info["serial"], false);
    assert!(info["nodes"].as_array().unwrap().iter().any(|n| n["id"] == mixer && n["kind"] == "layer" && n["blend"] == "Normal"));
    s.execute("grade.setMixer", json!({"node": mixer, "blend": "Screen", "opacity": 40})).unwrap();
    let info = s.execute("grade.info", json!({})).unwrap();
    assert!(info["nodes"].as_array().unwrap().iter().any(|n| n["id"] == mixer && n["blend"] == "Screen" && n["opacity"] == 40.0));
    assert!(s.execute("grade.setMixer", json!({"node": branch, "opacity": 40})).is_err(), "a corrector is not a mixer");
    let b2 = s.execute("grade.addBranch", json!({"mixer": mixer})).unwrap()["node"].as_u64().unwrap();
    // a link that would cycle is refused, the graph stays valid
    assert!(s.execute("grade.link", json!({"from": mixer, "to": b2, "input": 0})).is_err());
    s.execute("grade.unlink", json!({"to": mixer, "input": 2})).unwrap();
    s.execute("grade.removeNode", json!({"node": b2})).unwrap();
    s.execute("grade.removeNode", json!({"node": mixer})).unwrap();
    let info = s.execute("grade.info", json!({})).unwrap();
    assert_eq!(info["valid"], true);
    s.render_program(0.1).unwrap();
}

#[test]
fn hostile_parameters_are_errors_not_panics() {
    let (mut s, _) = demo();
    for (cmd, p) in [
        ("grade.removeNode", json!({"node": 999})),
        ("grade.removeNode", json!({"node": -1})),
        ("grade.removeNode", json!({})),
        ("grade.link", json!({"from": 0, "to": 1, "input": 4_000_000_000u64})),
        ("grade.moveNode", json!({"node": 2, "x": "nan", "y": 0})),
        ("grade.setMixer", json!({"node": 2, "blend": 10_000})),
        ("grade.addBranch", json!({"mixer": 2})),
        ("grade.resetNode", json!({"node": 0})),
        ("effects.setParam", json!({"clip": 1, "node": 4_000_000_000u64, "param": "exposure", "value": 1})),
    ] {
        assert!(s.execute(cmd, p.clone()).is_err(), "{cmd} {p}");
    }
    // commands that need a graph on a clip without one
    assert!(s.execute("grade.info", json!({})).unwrap()["active"] == false);
    s.execute("grade.addSerial", json!({})).unwrap();
    s.execute("grade.moveNode", json!({"node": 2, "x": 1e300, "y": -1e300})).unwrap();
    assert!(s.execute("grade.moveNode", json!({"node": 2, "x": f64::NAN, "y": 0.0})).is_err());
    s.execute("grade.setLabel", json!({"node": 2, "label": "x".repeat(10_000)})).unwrap();
    s.execute("grade.autoLayout", json!({})).unwrap();
    s.render_program(0.1).unwrap();
}
