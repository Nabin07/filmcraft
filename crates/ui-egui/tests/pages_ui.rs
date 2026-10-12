//! Headless UI tests of the page bar (Media · Cut · Edit · Color · Fairlight · Deliver) and the
//! Color page's node graph: a page is a mode plus a workspace, the bar follows changes made
//! elsewhere, and the node graph edits the selected clip's grade through `grade.*` commands.
//! Set `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` to also render offscreen with wgpu and write `pages-*.png`.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::dock::PanelKind;
use filmcraft_ui_egui::state::{Mode, Page};
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(s).with_control(rx);
        let snapshots = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if snapshots.is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, snapshots };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    fn has(&mut self, id: &str) -> bool {
        self.app().auto.elements.iter().any(|e| e.id == id)
    }

    fn snapshot(&mut self, name: &str) {
        let Some(dir) = self.snapshots.clone() else { return };
        self.frames(2);
        if let Ok(img) = self.harness.render() {
            let _ = std::fs::create_dir_all(&dir);
            img.save(dir.join(format!("pages-{name}.png"))).unwrap();
        }
    }

    fn click(&mut self, id: &str) {
        assert!(self.has(id), "no element `{id}`");
        self.ok("ui.click", json!({"id": id}));
        self.frames(2);
    }
}

#[test]
fn the_page_bar_switches_mode_and_workspace() {
    let mut d = Driver::new();
    for p in Page::ALL {
        assert!(d.has(&format!("page.{}", p.slug())), "page {:?} on the bar", p);
    }
    assert_eq!(d.app().ui.page, Page::Edit);
    d.click("page.color");
    d.snapshot("color-empty");
    assert_eq!(d.app().ui.page, Page::Color);
    assert_eq!(d.app().ui.mode, Mode::Edit);
    assert_eq!(d.app().ui.workspace, "Color");
    assert!(d.app().ui.dock.contains(PanelKind::NodeGraph), "the Color page has the node graph");
    assert!(d.app().ui.dock.contains(PanelKind::LumetriColor));
    d.click("page.deliver");
    assert_eq!(d.app().ui.mode, Mode::Export);
    d.click("page.media");
    assert_eq!(d.app().ui.mode, Mode::Import);
    d.click("page.cut");
    d.snapshot("cut");
    assert_eq!((d.app().ui.mode, d.app().ui.workspace.as_str()), (Mode::Edit, "Cut"));
    d.click("page.fairlight");
    assert_eq!(d.app().ui.workspace, "Audio");
    // the bar follows a workspace or mode changed elsewhere
    d.ok("ui.set", json!({"workspace": "Editing"}));
    assert_eq!(d.app().ui.page, Page::Edit);
    d.ok("ui.set", json!({"mode": "export"}));
    assert_eq!(d.app().ui.page, Page::Deliver);
    // ...and a page set by name or command
    d.ok("ui.set", json!({"page": "Color"}));
    assert_eq!(d.app().ui.page, Page::Color);
    d.ok("engine.execute", json!({"command": "window.page.edit", "params": {}}));
    assert_eq!((d.app().ui.page, d.app().ui.workspace.as_str()), (Page::Edit, "Editing"));
    // a workspace that is no page keeps the Edit-mode page it was reached from
    d.ok("ui.set", json!({"workspace": "Effects"}));
    assert_eq!(d.app().ui.page, Page::Edit);
    assert!(d.app().ui_error.is_none(), "{:?}", d.app().ui_error);
    // the serialized UI state carries the page
    let inspect = d.ok("ui.inspect", json!({}));
    assert_eq!(inspect["ui"]["page"], "edit");
}

#[test]
fn the_node_graph_grades_the_selected_clip() {
    let mut d = Driver::new();
    d.click("page.color");
    // select the first video clip: the node graph offers to start its grade
    let clip = d.app().session.active_sequence().unwrap().video_tracks[0].items[0].id;
    d.app().session.state.selection = vec![clip];
    d.frames(2);
    assert!(d.has("nodes.start"), "the node graph is shown");
    d.click("nodes.start");
    let g = d.app().session.active_sequence().unwrap().find_item(clip).unwrap().1.grade.clone().expect("a node grade");
    assert_eq!(g.correctors().len(), 1);
    let first = g.correctors()[0];
    assert_eq!(d.app().ui.grade_node, Some(first));
    assert!(d.has(&format!("nodes.node.{first}")));
    // the Lumetri panel edits the selected node
    assert!(d.app().auto.elements.iter().any(|e| e.id == "lumetri.section.Basic Correction" || e.id.starts_with("lumetri.")), "the corrector panel is up");
    d.click("nodes.add.serial");
    let g = d.app().session.active_sequence().unwrap().find_item(clip).unwrap().1.grade.clone().unwrap();
    assert_eq!(g.correctors().len(), 2);
    let second = g.correctors()[1];
    assert_eq!(d.app().ui.grade_node, Some(second), "the new node is selected");
    d.click("nodes.add.layer");
    d.snapshot("color-nodes");
    let g = d.app().session.active_sequence().unwrap().find_item(clip).unwrap().1.grade.clone().unwrap();
    assert!(g.serial_chain().is_none(), "a layer mixer was added");
    assert!(g.order().is_ok());
    // bypass through the toolbar, then delete the selected node
    let sel = d.app().ui.grade_node.unwrap();
    d.click("nodes.bypass");
    assert!(!d.app().session.active_sequence().unwrap().find_item(clip).unwrap().1.grade.as_ref().unwrap().node(sel).unwrap().enabled);
    d.click("nodes.delete");
    assert!(d.app().session.active_sequence().unwrap().find_item(clip).unwrap().1.grade.as_ref().unwrap().node(sel).is_none());
    // a parameter set with `node` lands on that node, not on the stack
    d.ok("engine.execute", json!({"command": "effects.setParam", "params": {"clip": clip.0, "node": first, "param": "exposure", "value": 1.5}}));
    let it = d.app().session.active_sequence().unwrap().find_item(clip).unwrap().1.clone();
    assert_eq!(it.grade.as_ref().unwrap().corrector(first).unwrap().f64_at("exposure", filmcraft_time::Tick(0)), 1.5);
    assert!(!it.effects.iter().any(|e| e.effect == "lumetri"));
    d.click("nodes.reset");
    assert!(d.app().session.active_sequence().unwrap().find_item(clip).unwrap().1.grade.is_none());
    assert!(d.app().ui_error.is_none(), "{:?}", d.app().ui_error);
}
