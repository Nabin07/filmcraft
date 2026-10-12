//! The Color page's node graph: the selected clip's grade ([`filmcraft_project::GradeGraph`]) as
//! nodes and wires. Click a node to select it (the Lumetri panel then edits that corrector), drag
//! it to move it, drag from an output port to an input port to wire it, right-click for the node
//! menu. The toolbar adds serial / parallel / layer nodes after the selected one. Everything goes
//! through `grade.*` commands, so the control channel and MCP can do the same.
//!
//! Automation ids: `nodes.add.serial|parallel|layer|branch`, `nodes.bypass`, `nodes.delete`,
//! `nodes.autoLayout`, `nodes.reset`, `nodes.start`, `nodes.node.<id>`, `nodes.port.out.<id>`,
//! `nodes.port.in.<id>.<input>`.

use egui::{Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use filmcraft_project::{ClipId, GradeGraph, GradeNodeKind, NodeId, TrackItem};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

const TOOLBAR_H: f32 = 28.0;
const PORT_R: f32 = 4.5;

/// The selected video clip, as the Lumetri panel picks it.
pub(crate) fn selected(app: &FilmcraftApp) -> Option<(ClipId, TrackItem)> {
    let seq = app.session.active_sequence()?;
    app.session.state.selection.iter().find_map(|c| {
        let (tid, it) = seq.find_item(*c)?;
        (seq.track(tid)?.kind == filmcraft_project::TrackKind::Video).then(|| (*c, it.clone()))
    })
}

/// The node the Lumetri panel edits for `it`: the selected node when it is one of the clip's
/// correctors, else the clip's first corrector. None: the clip has no node grade.
pub(crate) fn edited_node(app: &FilmcraftApp, it: &TrackItem) -> Option<NodeId> {
    let g = it.grade.as_ref()?;
    app.ui.grade_node.filter(|n| g.corrector(*n).is_some()).or_else(|| g.correctors().first().copied())
}

fn node_size(kind: &GradeNodeKind) -> egui::Vec2 {
    match kind {
        GradeNodeKind::Input | GradeNodeKind::Output => vec2(64.0, 36.0),
        GradeNodeKind::Corrector { .. } => vec2(104.0, 56.0),
        GradeNodeKind::Parallel | GradeNodeKind::Layer { .. } => vec2(84.0, 56.0),
    }
}

fn in_port(r: Rect, i: u32, n: u32) -> Pos2 {
    let step = r.height() / (n.max(1) as f32 + 1.0);
    pos2(r.min.x, r.min.y + step * (i as f32 + 1.0))
}

fn out_port(r: Rect) -> Pos2 {
    pos2(r.max.x, r.center().y)
}

#[derive(Clone, Copy)]
struct Wire {
    from: NodeId,
}

#[derive(Clone, Copy, Default)]
struct View {
    pan: egui::Vec2,
    /// The user panned: keep `pan` instead of fitting the graph.
    manual: bool,
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let Some((clip, it)) = selected(app) else {
        crate::dock::placeholder(ui, rect, &t, tl!("(select a clip to grade)"));
        return;
    };
    let Some(graph) = it.grade.clone() else {
        let r = Rect::from_center_size(rect.center(), vec2(180.0, 28.0));
        let resp = ui.put(r, egui::Button::new(tl!("Start node grade")));
        app.auto.add("nodes.start", r, "Start node grade");
        ui.painter().text(
            pos2(rect.center().x, r.min.y - 16.0),
            egui::Align2::CENTER_CENTER,
            tlf!("{name} has no node grade", name = it.name),
            Tokens::ui(11.0),
            t.text_faint,
        );
        if resp.clicked() {
            run(app, "grade.start", json!({"clip": clip.0}));
        }
        return;
    };
    let selected_node = edited_node(app, &it);
    if app.ui.grade_node != selected_node {
        app.ui.grade_node = selected_node;
    }
    let mut cmds: Vec<(&'static str, Value)> = Vec::new();

    // ---- toolbar
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), TOOLBAR_H));
    ui.painter().rect_filled(bar, 0.0, t.tab_bg);
    let mut bu = ui.new_child(
        egui::UiBuilder::new().max_rect(bar.shrink2(vec2(6.0, 3.0))).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt("nodes-toolbar"),
    );
    bu.spacing_mut().item_spacing.x = 4.0;
    let sel_is_mixer = selected_node.and_then(|n| graph.node(n)).is_some_and(|n| matches!(n.kind, GradeNodeKind::Parallel | GradeNodeKind::Layer { .. }));
    let sel_bypassed = selected_node.and_then(|n| graph.node(n)).is_some_and(|n| !n.enabled);
    let tool = |bu: &mut egui::Ui, app: &mut FilmcraftApp, id: &str, label: &str, tip: &str, enabled: bool| -> bool {
        let r = bu.add_enabled(enabled, egui::Button::new(egui::RichText::new(label).size(11.0)).min_size(vec2(0.0, 20.0))).on_hover_text(tip);
        app.auto.add(&format!("nodes.{id}"), r.rect, tip);
        r.clicked()
    };
    let after = selected_node.map(|n| json!(n)).unwrap_or(Value::Null);
    if tool(&mut bu, app, "add.serial", "+ Serial", tl!("Add a serial node after the selected node"), true) {
        cmds.push(("grade.addSerial", json!({"clip": clip.0, "after": after})));
    }
    if tool(&mut bu, app, "add.parallel", "+ Parallel", tl!("Add a parallel mixer with a new branch after the selected node"), true) {
        cmds.push(("grade.addParallel", json!({"clip": clip.0, "at": after})));
    }
    if tool(&mut bu, app, "add.layer", "+ Layer", tl!("Add a layer mixer with a new branch after the selected node"), true) {
        cmds.push(("grade.addLayer", json!({"clip": clip.0, "at": after})));
    }
    if tool(&mut bu, app, "add.branch", "+ Branch", tl!("Add a branch into the selected mixer"), sel_is_mixer) {
        cmds.push(("grade.addBranch", json!({"clip": clip.0, "mixer": after})));
    }
    bu.separator();
    let can_edit = selected_node.is_some_and(|n| n > filmcraft_project::OUTPUT_ID);
    if tool(&mut bu, app, "bypass", if sel_bypassed { "Enable" } else { "Bypass" }, tl!("Bypass / enable the selected node"), can_edit) {
        cmds.push(("grade.setEnabled", json!({"clip": clip.0, "node": after})));
    }
    if tool(&mut bu, app, "delete", "Delete", tl!("Delete the selected node"), can_edit) {
        cmds.push(("grade.removeNode", json!({"clip": clip.0, "node": after})));
    }
    bu.separator();
    if tool(&mut bu, app, "autoLayout", "Tidy", tl!("Lay the nodes out again"), true) {
        cmds.push(("grade.autoLayout", json!({"clip": clip.0})));
    }
    if tool(&mut bu, app, "reset", "Remove", tl!("Remove the node grade from the clip"), true) {
        cmds.push(("grade.reset", json!({"clip": clip.0})));
    }
    let title = tlf!("{name} · {n} nodes", name = it.name, n = graph.correctors().len());
    bu.with_layout(egui::Layout::right_to_left(egui::Align::Center), |bu| {
        bu.add(egui::Label::new(egui::RichText::new(title).size(11.0).color(t.text_dim)).truncate());
    });

    // ---- canvas
    let canvas = Rect::from_min_max(pos2(rect.min.x, bar.max.y), rect.max);
    let p = ui.painter_at(canvas);
    p.rect_filled(canvas, 0.0, t.plot_bg);
    // dots
    let dot = t.plot_grid;
    let step = 24.0;
    let mut gx = canvas.min.x + 8.0;
    while gx < canvas.max.x {
        let mut gy = canvas.min.y + 8.0;
        while gy < canvas.max.y {
            p.circle_filled(pos2(gx, gy), 0.8, dot);
            gy += step;
        }
        gx += step;
    }
    // The view: fitted to the graph until the user pans it (then kept, per clip, across frames).
    // A wide graph is drawn smaller so the whole chain stays in sight.
    let view_id = egui::Id::new(("nodes-view", clip.0));
    let mut view: View = ui.ctx().data(|d| d.get_temp(view_id)).unwrap_or_default();
    let bg = ui.interact(canvas, egui::Id::new(("nodes-canvas", clip.0)), Sense::click_and_drag());
    let mut bb = Rect::NOTHING;
    for n in &graph.nodes {
        bb = bb.union(Rect::from_min_size(pos2(n.x, n.y), node_size(&n.kind)));
    }
    let bb = if bb.is_positive() { bb } else { Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0)) };
    let zoom = ((canvas.width() - 48.0) / bb.width().max(1.0)).min((canvas.height() - 48.0) / bb.height().max(1.0)).clamp(0.6, 1.0);
    if !view.manual {
        view.pan = (canvas.size() - bb.size() * zoom) / 2.0 - bb.min.to_vec2() * zoom;
    }
    if bg.dragged() {
        view.pan += bg.drag_delta();
        view.manual = true;
    }
    if bg.double_clicked() {
        view.manual = false;
    }
    if bg.clicked() {
        app.ui.grade_node = None;
        app.ui.focused = crate::dock::PanelKind::NodeGraph;
    }
    let origin = canvas.min + view.pan;
    let place = |n: &filmcraft_project::GradeNode| Rect::from_min_size(origin + vec2(n.x, n.y) * zoom, node_size(&n.kind) * zoom);

    // wires
    let wire_id = egui::Id::new(("nodes-wire", clip.0));
    let dragging: Option<Wire> = ui.ctx().data(|d| d.get_temp(wire_id));
    for l in &graph.links {
        let (Some(a), Some(b)) = (graph.node(l.from), graph.node(l.to)) else { continue };
        let from = out_port(place(a));
        let to = in_port(place(b), l.input, graph.input_count(l.to));
        draw_wire(&p, from, to, Stroke::new(1.5, t.text_dim));
    }

    // nodes
    let mut hover_in: Option<(NodeId, u32)> = None;
    let pointer = ui.ctx().pointer_latest_pos();
    let mut select: Option<Option<NodeId>> = None;
    for n in &graph.nodes {
        let r = place(n);
        if !canvas.intersects(r.expand(40.0)) {
            continue;
        }
        let selected = Some(n.id) == selected_node;
        let is_io = matches!(n.kind, GradeNodeKind::Input | GradeNodeKind::Output);
        let resp = ui.interact(r, egui::Id::new(("nodes-node", clip.0, n.id)), Sense::click_and_drag());
        app.auto.add(&format!("nodes.node.{}", n.id), r, &node_title(&graph, n));
        let fill = match &n.kind {
            GradeNodeKind::Input | GradeNodeKind::Output => t.control_surface,
            GradeNodeKind::Corrector { .. } => t.control_well,
            GradeNodeKind::Parallel | GradeNodeKind::Layer { .. } => t.field_bg,
        };
        p.rect_filled(r, 6.0, fill);
        let border = if selected {
            t.accent
        } else if resp.hovered() {
            t.text_dim
        } else {
            t.control_border
        };
        p.rect_stroke(r, 6.0, Stroke::new(if selected { 2.0 } else { 1.0 }, border), egui::StrokeKind::Outside);
        // body
        let text_col = if n.enabled { t.text } else { t.text_faint };
        match &n.kind {
            GradeNodeKind::Input => {
                p.text(r.center(), egui::Align2::CENTER_CENTER, tl!("Source"), Tokens::ui(11.0 * zoom.max(0.8)), text_col);
            }
            GradeNodeKind::Output => {
                p.text(r.center(), egui::Align2::CENTER_CENTER, tl!("Output"), Tokens::ui(11.0 * zoom.max(0.8)), text_col);
            }
            GradeNodeKind::Corrector { effect } => {
                let num = graph.number_of(n.id).map(|k| format!("{k:02}")).unwrap_or_default();
                p.text(pos2(r.min.x + 10.0 * zoom, r.min.y + 8.0 * zoom), egui::Align2::LEFT_TOP, num, Tokens::ui(16.0 * zoom.max(0.7)), text_col);
                if zoom >= 0.8 || !n.label.is_empty() {
                    let label = if n.label.is_empty() { tl!("Corrector").to_string() } else { n.label.clone() };
                    let mut job =
                        egui::text::LayoutJob::simple_singleline(label, Tokens::ui(10.0 * zoom.max(0.8)), if n.enabled { t.text_dim } else { t.text_faint });
                    job.wrap.max_width = r.width() - 16.0 * zoom;
                    job.wrap.max_rows = 1;
                    let g = p.layout_job(job);
                    p.galley(pos2(r.min.x + 10.0 * zoom, r.max.y - 8.0 * zoom - g.size().y), g, t.text_dim);
                }
                // what the node does: little dots for the sections in use
                let mut dx = r.max.x - 10.0;
                for (prm, col) in [
                    ("vignette_on", t.text_faint),
                    ("hsl_on", Color32::from_rgb(0xd8, 0x8a, 0x3c)),
                    ("curves_on", Color32::from_rgb(0x6c, 0xa8, 0xe0)),
                    ("wheels_on", Color32::from_rgb(0xc0, 0x6c, 0xd8)),
                ] {
                    let on = effect.param(prm).and_then(|v| v.value.as_bool()).unwrap_or(false) && touched(effect, prm);
                    if on {
                        p.circle_filled(pos2(dx, r.min.y + 12.0), 2.5, col);
                        dx -= 8.0;
                    }
                }
            }
            GradeNodeKind::Parallel => {
                icons::paint(&p, Rect::from_center_size(pos2(r.center().x, r.min.y + 18.0 * zoom), vec2(16.0, 16.0) * zoom), Icon::Nodes, text_col);
                p.text(pos2(r.center().x, r.max.y - 12.0 * zoom), egui::Align2::CENTER_CENTER, tl!("Parallel"), Tokens::ui(10.0 * zoom.max(0.8)), t.text_dim);
            }
            GradeNodeKind::Layer { blend, opacity } => {
                icons::paint(&p, Rect::from_center_size(pos2(r.center().x, r.min.y + 18.0 * zoom), vec2(16.0, 16.0) * zoom), Icon::Nodes, text_col);
                let mode = filmcraft_project::effect::BLEND_MODES.get(*blend as usize).copied().unwrap_or("Normal");
                p.text(
                    pos2(r.center().x, r.max.y - 12.0 * zoom),
                    egui::Align2::CENTER_CENTER,
                    format!("{mode} {opacity:.0}%"),
                    Tokens::ui(9.0 * zoom.max(0.8)),
                    t.text_dim,
                );
            }
        }
        if !n.enabled {
            p.line_segment([r.left_bottom() + vec2(6.0, -6.0), r.right_top() + vec2(-6.0, 6.0)], Stroke::new(1.5, Color32::from_rgb(0xd8, 0x4a, 0x4a)));
        }
        // ports
        let n_in = graph.input_count(n.id);
        for i in 0..n_in {
            let c = in_port(r, i, n_in);
            let pr = Rect::from_center_size(c, vec2(14.0, 14.0));
            app.auto.add(&format!("nodes.port.in.{}.{i}", n.id), pr, "input");
            let hot = pointer.is_some_and(|q| pr.contains(q));
            if hot {
                hover_in = Some((n.id, i));
            }
            p.circle(c, PORT_R, if hot { t.accent } else { t.control_handle_bg }, Stroke::new(1.0, t.control_handle));
        }
        if !matches!(n.kind, GradeNodeKind::Output) {
            let c = out_port(r);
            let pr = Rect::from_center_size(c, vec2(14.0, 14.0));
            let port = ui.interact(pr, egui::Id::new(("nodes-out", clip.0, n.id)), Sense::drag());
            app.auto.add(&format!("nodes.port.out.{}", n.id), pr, "output");
            if port.drag_started() {
                ui.ctx().data_mut(|d| d.insert_temp(wire_id, Wire { from: n.id }));
            }
            p.circle(
                c,
                PORT_R,
                if port.hovered() || dragging.is_some_and(|w| w.from == n.id) { t.accent } else { t.control_handle_bg },
                Stroke::new(1.0, t.control_handle),
            );
        }
        // select / move
        if resp.clicked() || resp.drag_started() {
            select = Some(Some(n.id));
        }
        if resp.dragged() && !is_io || resp.dragged() && is_io {
            let d = resp.drag_delta() / zoom;
            if d != egui::Vec2::ZERO {
                let begin = resp.drag_started();
                view.manual = true;
                cmds.push(("grade.moveNode", json!({"clip": clip.0, "node": n.id, "x": n.x + d.x, "y": n.y + d.y, "merge": true, "begin": begin})));
            }
        }
        if resp.double_clicked() && !is_io {
            cmds.push(("grade.setEnabled", json!({"clip": clip.0, "node": n.id})));
        }
        resp.context_menu(|mu| {
            mu.set_min_width(180.0);
            if mu.button(tl!("Add Serial Node")).clicked() {
                cmds.push(("grade.addSerial", json!({"clip": clip.0, "after": n.id})));
                mu.close();
            }
            if mu.button(tl!("Add Serial Node Before")).clicked() {
                cmds.push(("grade.addSerial", json!({"clip": clip.0, "before": n.id})));
                mu.close();
            }
            if mu.button(tl!("Add Parallel Node")).clicked() {
                cmds.push(("grade.addParallel", json!({"clip": clip.0, "at": n.id})));
                mu.close();
            }
            if mu.button(tl!("Add Layer Node")).clicked() {
                cmds.push(("grade.addLayer", json!({"clip": clip.0, "at": n.id})));
                mu.close();
            }
            if matches!(n.kind, GradeNodeKind::Parallel | GradeNodeKind::Layer { .. }) && mu.button(tl!("Add Branch")).clicked() {
                cmds.push(("grade.addBranch", json!({"clip": clip.0, "mixer": n.id})));
                mu.close();
            }
            if let GradeNodeKind::Layer { blend, opacity } = &n.kind {
                mu.separator();
                let mode = filmcraft_project::effect::BLEND_MODES.get(*blend as usize).copied().unwrap_or("Normal");
                mu.menu_button(tlf!("Blend: {mode}", mode), |mm| {
                    for (i, name) in filmcraft_project::effect::BLEND_MODES.iter().enumerate() {
                        if mm.selectable_label(i as u32 == *blend, *name).clicked() {
                            cmds.push(("grade.setMixer", json!({"clip": clip.0, "node": n.id, "blend": i})));
                            mm.close();
                        }
                    }
                });
                let mut op = *opacity as f32;
                if mu.add(egui::Slider::new(&mut op, 0.0..=100.0).text(tl!("Opacity"))).changed() {
                    cmds.push(("grade.setMixer", json!({"clip": clip.0, "node": n.id, "opacity": op})));
                }
            }
            if !is_io {
                mu.separator();
                if mu.button(if n.enabled { tl!("Bypass Node") } else { tl!("Enable Node") }).clicked() {
                    cmds.push(("grade.setEnabled", json!({"clip": clip.0, "node": n.id})));
                    mu.close();
                }
                if matches!(n.kind, GradeNodeKind::Corrector { .. }) {
                    if mu.button(tl!("Reset Node")).clicked() {
                        cmds.push(("grade.resetNode", json!({"clip": clip.0, "node": n.id})));
                        mu.close();
                    }
                    let key = egui::Id::new(("nodes-label", clip.0, n.id));
                    let mut label: String = mu.ctx().data(|d| d.get_temp(key)).unwrap_or_else(|| n.label.clone());
                    let te = mu.add(egui::TextEdit::singleline(&mut label).hint_text(tl!("Label")).desired_width(150.0));
                    if te.changed() {
                        mu.ctx().data_mut(|d| d.insert_temp(key, label.clone()));
                    }
                    if te.lost_focus() {
                        cmds.push(("grade.setLabel", json!({"clip": clip.0, "node": n.id, "label": label})));
                        mu.ctx().data_mut(|d| d.remove::<String>(key));
                    }
                }
                if mu.button(tl!("Delete Node")).clicked() {
                    cmds.push(("grade.removeNode", json!({"clip": clip.0, "node": n.id})));
                    mu.close();
                }
            }
        });
    }
    // a wire being dragged: from its port to the pointer; released on an input port it connects
    if let Some(w) = dragging {
        if let (Some(from), Some(q)) = (graph.node(w.from), pointer) {
            draw_wire(&p, out_port(place(from)), q, Stroke::new(1.5, t.accent));
        }
        let released = ui.ctx().input(|i| i.pointer.any_released());
        if released {
            ui.ctx().data_mut(|d| d.remove::<Wire>(wire_id));
            if let Some((to, input)) = hover_in
                && to != w.from
            {
                cmds.push(("grade.link", json!({"clip": clip.0, "from": w.from, "to": to, "input": input})));
            }
        }
        ui.ctx().request_repaint();
    }
    ui.ctx().data_mut(|d| d.insert_temp(view_id, view));
    if let Some(sel) = select {
        app.ui.grade_node = sel;
        app.ui.focused = crate::dock::PanelKind::NodeGraph;
    }
    // added nodes become the selection, so the corrector panel moves to them
    for (cmd, params) in cmds {
        let adds = matches!(cmd, "grade.addSerial" | "grade.addParallel" | "grade.addLayer" | "grade.addBranch" | "grade.start");
        match app.session.execute(cmd, params) {
            Ok(v) if adds => {
                if let Some(n) = v["node"].as_u64().and_then(|n| u32::try_from(n).ok()) {
                    app.ui.grade_node = Some(n);
                }
            }
            Ok(_) => {}
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}

fn run(app: &mut FilmcraftApp, cmd: &str, params: Value) {
    match app.session.execute(cmd, params) {
        Ok(v) => {
            if let Some(n) = v["node"].as_u64().and_then(|n| u32::try_from(n).ok()) {
                app.ui.grade_node = Some(n);
            }
        }
        Err(e) => app.ui.status = e.to_string(),
    }
}

fn node_title(g: &GradeGraph, n: &filmcraft_project::GradeNode) -> String {
    match &n.kind {
        GradeNodeKind::Input => "Source".into(),
        GradeNodeKind::Output => "Output".into(),
        GradeNodeKind::Corrector { .. } => {
            format!("Node {:02}{}", g.number_of(n.id).unwrap_or(0), if n.label.is_empty() { String::new() } else { format!(" {}", n.label) })
        }
        GradeNodeKind::Parallel => "Parallel mixer".into(),
        GradeNodeKind::Layer { .. } => "Layer mixer".into(),
    }
}

/// Whether a Lumetri section carries any non-default value (so the node shows its dot).
fn touched(e: &filmcraft_project::EffectInstance, section_switch: &str) -> bool {
    let Some(def) = e.def() else { return false };
    let group = def.param(section_switch).and_then(|p| p.group);
    def.params.iter().filter(|p| p.group == group && p.id != section_switch).any(|p| e.param(p.id).is_some_and(|v| v.value != p.default || v.is_animated()))
}

fn draw_wire(p: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    let dx = ((to.x - from.x).abs() * 0.5).clamp(24.0, 120.0);
    let shape =
        egui::epaint::CubicBezierShape::from_points_stroke([from, pos2(from.x + dx, from.y), pos2(to.x - dx, to.y), to], false, Color32::TRANSPARENT, stroke);
    p.add(shape);
}
