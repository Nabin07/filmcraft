//! The page bar along the bottom of the window: Media · Cut · Edit · Color · Fairlight · Deliver,
//! the Resolve-style workflow. A page is a mode (Import / Edit / Export) plus a workspace, so the
//! Premiere-style workspaces and modes keep working underneath and the control channel sees the
//! page as `ui.page`.

use egui::{Rect, Sense, Stroke, pos2, vec2};

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::state::{Mode, Page};
use crate::theme::Tokens;

pub const HEIGHT: f32 = 44.0;

impl Page {
    pub const ALL: [Page; 6] = [Page::Media, Page::Cut, Page::Edit, Page::Color, Page::Fairlight, Page::Deliver];

    pub fn title(self) -> &'static str {
        match self {
            Page::Media => "Media",
            Page::Cut => "Cut",
            Page::Edit => "Edit",
            Page::Color => "Color",
            Page::Fairlight => "Fairlight",
            Page::Deliver => "Deliver",
        }
    }
    /// The command-id key: `window.page.<slug>`, `ui.set {page}`.
    pub fn slug(self) -> &'static str {
        match self {
            Page::Media => "media",
            Page::Cut => "cut",
            Page::Edit => "edit",
            Page::Color => "color",
            Page::Fairlight => "fairlight",
            Page::Deliver => "deliver",
        }
    }
    pub fn from_name(s: &str) -> Option<Page> {
        let n = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|p| p.slug() == n || p.title().eq_ignore_ascii_case(&n))
    }
    pub fn icon(self) -> Icon {
        match self {
            Page::Media => Icon::Folder,
            Page::Cut => Icon::Razor,
            Page::Edit => Icon::Sequence,
            Page::Color => Icon::ColorWheel,
            Page::Fairlight => Icon::Audio,
            Page::Deliver => Icon::Export,
        }
    }
    /// The mode the page runs in.
    pub fn mode(self) -> Mode {
        match self {
            Page::Media => Mode::Import,
            Page::Deliver => Mode::Export,
            _ => Mode::Edit,
        }
    }
    /// The workspace an Edit-mode page lays out (None: the page is not a dock layout).
    pub fn workspace(self) -> Option<&'static str> {
        match self {
            Page::Cut => Some("Cut"),
            Page::Edit => Some("Editing"),
            Page::Color => Some("Color"),
            Page::Fairlight => Some("Audio"),
            Page::Media | Page::Deliver => None,
        }
    }
}

impl FilmcraftApp {
    /// Switch to a page: its mode, and its workspace when it has one.
    pub fn set_page(&mut self, page: Page) {
        self.ui.page = page;
        self.ui.mode = page.mode();
        if let Some(w) = page.workspace()
            && let Some(name) = crate::dock::find(&self.workspaces, w)
            && self.ui.workspace != name
        {
            self.set_workspace(&name);
        }
    }

    /// The page shown now, following mode / workspace changes made elsewhere (the header's Home, a
    /// menu, the control channel). A customised workspace keeps the Edit-mode page it was reached from.
    pub fn current_page(&self) -> Page {
        match self.ui.mode {
            Mode::Import => Page::Media,
            Mode::Export => Page::Deliver,
            Mode::Edit => Page::ALL
                .into_iter()
                .find(|p| p.workspace().is_some_and(|w| w.eq_ignore_ascii_case(&self.ui.workspace)))
                .unwrap_or(if self.ui.page.mode() == Mode::Edit { self.ui.page } else { Page::Edit }),
        }
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.header_bg);
    p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, egui::Color32::BLACK));
    let current = app.current_page();
    if app.ui.page != current {
        app.ui.page = current;
    }
    // pages, centred
    let pitch = 92.0_f32.min((rect.width() - 240.0).max(300.0) / Page::ALL.len() as f32);
    let total = pitch * Page::ALL.len() as f32;
    let mut x = rect.center().x - total / 2.0;
    for page in Page::ALL {
        let r = Rect::from_min_size(pos2(x, rect.min.y + 2.0), vec2(pitch, rect.height() - 4.0));
        let resp = ui.interact(r, egui::Id::new(("page", page.slug())), Sense::click()).on_hover_text(crate::i18n::t(page.title()));
        app.auto.add(&format!("page.{}", page.slug()), r, page.title());
        let active = page == current;
        if resp.hovered() && !active {
            p.rect_filled(r.shrink2(vec2(4.0, 2.0)), 6.0, t.hover);
        }
        let col = if active {
            t.accent
        } else if resp.hovered() {
            t.text
        } else {
            t.text_dim
        };
        let icon = Rect::from_center_size(pos2(r.center().x, r.min.y + 15.0), vec2(18.0, 18.0));
        icons::paint(&p, icon, page.icon(), col);
        let label = crate::i18n::t(page.title());
        p.text(pos2(r.center().x, r.max.y - 8.0), egui::Align2::CENTER_CENTER, label, Tokens::ui(10.0), col);
        if active {
            p.line_segment([pos2(r.min.x + 10.0, rect.max.y - 1.0), pos2(r.max.x - 10.0, rect.max.y - 1.0)], Stroke::new(2.0, t.accent));
        }
        if resp.clicked() {
            app.set_page(page);
        }
        x += pitch;
    }
    // left: the app, right: the sequence being edited
    p.text(pos2(rect.min.x + 14.0, rect.center().y), egui::Align2::LEFT_CENTER, "FilmCraft", Tokens::ui(12.0), t.text_faint);
    if let Some(name) = app.session.state.active_sequence.and_then(|s| app.session.project.item(s)).map(|i| i.name.clone()) {
        let g = p.layout_no_wrap(name, Tokens::ui(11.0), t.text_dim);
        if rect.width() - total > 2.0 * (g.size().x + 40.0) {
            p.galley(pos2(rect.max.x - 14.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, t.text_dim);
        }
    }
}
