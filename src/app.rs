//! The whole window: header (title, pin, close), tab strip, editor and footer toolbar.
//! Everything is laid out in absolute rects inside the frameless window and painted by hand.

use std::path::PathBuf;
use std::sync::mpsc;

use egui::{Align2, Color32, CursorIcon, Id, Pos2, Rect, ResizeDirection, Response, Sense, Stroke, Vec2, ViewportCommand};

use crate::icons;
use crate::store::{FontStyle, Note, State};
use crate::theme::{self, ThemeKind, Tokens};

const PAD: f32 = 12.0;
const HEADER_H: f32 = 36.0;
const TABS_H: f32 = 36.0;
const FOOTER_H: f32 = 42.0;
const RESIZE_EDGE: f32 = 6.0;
const SAVE_DELAY: f64 = 0.4;
const TOAST_SECS: f64 = 4.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Popup {
    Font,
    More,
}

/// What an Undo in the toast brings back.
enum Undo {
    Cleared { id: u64, title: String, text: String },
    Closed { index: usize, note: Note },
}

struct Toast {
    text: String,
    until: f64,
    undo: Option<Undo>,
}

/// Keeps the pin in step with Hyprland, which (unlike winit on Wayland) can pin a window.
#[cfg(target_os = "linux")]
struct WmSync {
    hypr: crate::hyprland::Hyprland,
    /// Window state changes seen on the compositor side (SUPER+O, SUPER+T, moving, resizing).
    observed: mpsc::Receiver<crate::hyprland::WindowState>,
    /// The saved size and position still to be restored once the window floats at start-up.
    restore: bool,
    /// A pin state still to be sent, and until when to retry (the window may not be mapped yet).
    pending: Option<(bool, f64)>,
    /// Observations older than our own last change are stale until this time.
    ignore_until: f64,
}

pub struct App {
    pub state: State,
    path: Option<PathBuf>,
    dirty_since: Option<f64>,
    fonts_ready: bool,
    applied_theme: Option<ThemeKind>,
    popup: Option<(Popup, Rect)>,
    /// Tab index, edited title, and whether the field still needs to grab focus.
    renaming: Option<(usize, String, bool)>,
    /// The active tab the strip last scrolled into view (scrolling only follows changes).
    scrolled_to: Option<u64>,
    toast: Option<Toast>,
    saves: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    /// Asks the editor to take keyboard focus on the next frame (after switching or adding a tab).
    focus_editor: bool,
    #[cfg(target_os = "linux")]
    wm: Option<WmSync>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, state: State, path: Option<PathBuf>) -> Self {
        #[allow(unused_mut)]
        let mut app = Self::with_state(&cc.egui_ctx, state, path);
        #[cfg(target_os = "linux")]
        if let Some(hypr) = crate::hyprland::Hyprland::detect() {
            let (tx, observed) = mpsc::channel();
            let (watcher, ctx) = (hypr.clone(), cc.egui_ctx.clone());
            std::thread::spawn(move || {
                let mut last = None;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let Some(now) = watcher.window_state() else { continue };
                    if last != Some(now) {
                        last = Some(now);
                        if tx.send(now).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                }
            });
            app.wm = Some(WmSync { hypr, observed, pending: Some((app.state.pinned, f64::NAN)), restore: true, ignore_until: 0.0 });
        }
        app
    }

    /// `path: None` keeps everything in memory (tests).
    pub fn with_state(ctx: &egui::Context, mut state: State, path: Option<PathBuf>) -> Self {
        state.fix_up();
        egui_extras::install_image_loaders(ctx);
        Self {
            state,
            path,
            dirty_since: None,
            fonts_ready: false,
            applied_theme: None,
            popup: None,
            renaming: None,
            scrolled_to: None,
            toast: None,
            saves: None,
            focus_editor: true,
            #[cfg(target_os = "linux")]
            wm: None,
        }
    }

    fn tokens(&self) -> Tokens {
        Tokens::of(self.state.theme)
    }

    fn mark_dirty(&mut self, now: f64) {
        self.dirty_since.get_or_insert(now);
    }

    pub fn save_now(&mut self) {
        self.dirty_since = None;
        if let Some(path) = &self.path
            && let Err(e) = self.state.save(path)
        {
            eprintln!("scratchpad: could not save {}: {e}", path.display());
        }
    }

    fn toast(&mut self, now: f64, text: impl Into<String>, undo: Option<Undo>) {
        self.toast = Some(Toast { text: text.into(), until: now + TOAST_SECS, undo });
    }

    // ---- actions (also used by keyboard shortcuts and tests) -------------------------------

    pub fn add_note(&mut self, now: f64) {
        self.state.add_note();
        self.focus_editor = true;
        self.mark_dirty(now);
    }

    pub fn close_note(&mut self, i: usize, now: f64) {
        let note = self.state.notes[i].clone();
        self.state.close_note(i);
        if !note.text.is_empty() {
            let msg = format!("Closed “{}”", note.title);
            self.toast(now, msg, Some(Undo::Closed { index: i, note }));
        }
        self.renaming = None;
        self.mark_dirty(now);
    }

    pub fn toggle_hidden(&mut self, now: f64) {
        let note = self.state.current_mut();
        note.hidden = !note.hidden;
        self.mark_dirty(now);
    }

    pub fn toggle_pin(&mut self, ctx: &egui::Context, now: f64) {
        self.state.pinned = !self.state.pinned;
        self.mark_dirty(now);
        #[cfg(target_os = "linux")]
        if let Some(wm) = &mut self.wm {
            wm.pending = Some((self.state.pinned, now + 2.0));
            return;
        }
        let level = if self.state.pinned { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(level));
    }

    pub fn copy(&mut self, ctx: &egui::Context, now: f64) {
        let text = self.state.current().text.clone();
        if text.is_empty() {
            self.toast(now, "Nothing to copy", None);
            return;
        }
        ctx.copy_text(text);
        self.toast(now, "Copied to clipboard", None);
    }

    pub fn clear(&mut self, now: f64) {
        let note = self.state.current_mut();
        let (id, title) = (note.id, note.title.clone());
        let text = std::mem::take(&mut note.text);
        if !text.is_empty() {
            self.toast(now, "Cleared", Some(Undo::Cleared { id, title, text }));
            self.mark_dirty(now);
        }
    }

    /// Opens the native save dialog on a worker thread so the window keeps drawing.
    pub fn export(&mut self) {
        let note = self.state.current().clone();
        let (tx, rx) = mpsc::channel();
        self.saves = Some(rx);
        let run = move || {
            let name = format!("{}.txt", sanitize(&note.title));
            let picked = rfd::FileDialog::new()
                .set_title("Save note")
                .set_file_name(name)
                .add_filter("Text", &["txt", "md"])
                .add_filter("All files", &["*"])
                .save_file();
            if let Some(path) = picked {
                let _ = tx.send(std::fs::write(&path, note.text).map(|_| path).map_err(|e| e.to_string()));
            }
        };
        // AppKit dialogs must run on the main thread; elsewhere a worker keeps the window drawing.
        if cfg!(target_os = "macos") {
            run();
        } else {
            std::thread::spawn(run);
        }
    }

    fn undo(&mut self, undo: Undo, now: f64) {
        match undo {
            Undo::Cleared { id, title, text } => match self.state.notes.iter().position(|n| n.id == id) {
                // Never overwrite something typed since: then the text comes back as its own note.
                Some(i) if self.state.notes[i].text.is_empty() => {
                    self.state.notes[i].text = text;
                    self.state.active = i;
                }
                _ => {
                    let mut note = Note::new(title);
                    note.id = self.state.fresh_id();
                    note.text = text;
                    self.state.notes.push(note);
                    self.state.active = self.state.notes.len() - 1;
                }
            },
            Undo::Closed { index, note } => {
                let index = index.min(self.state.notes.len());
                // Closing the last note left an empty placeholder: replace it.
                if self.state.notes.len() == 1 && self.state.notes[0].text.is_empty() {
                    self.state.notes.clear();
                }
                let index = index.min(self.state.notes.len());
                self.state.notes.insert(index, note);
                self.state.active = index;
            }
        }
        self.toast = None;
        self.focus_editor = true;
        self.mark_dirty(now);
    }

    fn shortcuts(&mut self, ctx: &egui::Context, now: f64) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let cmd = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let cmd_shift = |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
        if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::N)) || i.consume_shortcut(&cmd(Key::T))) {
            self.add_note(now);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::W))) {
            self.close_note(self.state.active, now);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::S))) {
            self.export();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::C))) {
            self.copy(ctx, now);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::H))) {
            self.toggle_hidden(now);
        }
        let n = self.state.notes.len();
        // Shift first: consume_shortcut ignores extra modifiers, so Ctrl+Tab would also match it.
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab))) {
            self.state.active = (self.state.active + n - 1) % n;
            self.focus_editor = true;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::CTRL, Key::Tab))) {
            self.state.active = (self.state.active + 1) % n;
            self.focus_editor = true;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.popup = None;
        }
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn window_frame(&self, ui: &egui::Ui, rect: Rect, t: &Tokens) {
        let p = ui.painter();
        // A soft vertical gradient: a mesh of two colours, clipped by the rounded body drawn on top of it.
        p.rect_filled(rect, t.radius, t.bg_bottom);
        let mut mesh = egui::Mesh::default();
        let fade = Rect::from_min_max(Pos2::new(rect.min.x + 1.0, rect.min.y + t.radius), Pos2::new(rect.max.x - 1.0, rect.max.y - t.radius));
        mesh.colored_vertex(fade.left_top(), t.bg_top);
        mesh.colored_vertex(fade.right_top(), t.bg_top);
        mesh.colored_vertex(fade.left_bottom(), t.bg_bottom);
        mesh.colored_vertex(fade.right_bottom(), t.bg_bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        p.add(mesh);
        p.rect_filled(
            Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + t.radius)),
            egui::CornerRadius { nw: t.radius as u8, ne: t.radius as u8, sw: 0, se: 0 },
            t.bg_top,
        );
        p.rect_stroke(rect, t.radius, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    }

    fn in_resize_band(&self, ui: &egui::Ui) -> bool {
        let window = ui.ctx().content_rect();
        ui.input(|i| i.pointer.press_origin()).is_some_and(|p| !window.shrink(RESIZE_EDGE).contains(p))
    }

    /// Lets the borderless window be resized from its edges and corners.
    fn resize_edges(&self, ui: &egui::Ui, rect: Rect) {
        let Some(pos) = ui.ctx().pointer_hover_pos() else { return };
        if !rect.contains(pos) {
            return;
        }
        let e = RESIZE_EDGE;
        let (l, r) = (pos.x < rect.left() + e, pos.x > rect.right() - e);
        let (t, b) = (pos.y < rect.top() + e, pos.y > rect.bottom() - e);
        let dir = match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return,
        };
        let icon = match dir {
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
            ResizeDirection::West | ResizeDirection::East => CursorIcon::ResizeHorizontal,
            _ => CursorIcon::ResizeVertical,
        };
        ui.ctx().set_cursor_icon(icon);
        if ui.input(|i| i.pointer.primary_pressed()) {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        // Empty header space moves the window.
        let drag = ui.interact(rect, Id::new("drag-header"), Sense::click_and_drag());
        // A press in the resize band belongs to resize_edges, not to the window drag.
        if drag.drag_started() && !self.in_resize_band(ui) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        ui.painter().text(
            Pos2::new(rect.left() + PAD + 2.0, rect.center().y + 2.0),
            Align2::LEFT_CENTER,
            "Scratchpad",
            theme::semibold(14.5),
            t.title,
        );
        let close = Rect::from_center_size(Pos2::new(rect.right() - PAD - 10.0, rect.center().y + 2.0), Vec2::splat(24.0));
        let resp = ui.interact(close, Id::new("close"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close window"));
        let fill = if resp.hovered() { t.text_muted } else { t.close_fill };
        ui.painter().circle_filled(close.center(), 9.5, fill);
        icons::paint(ui, close, "x", 12.0, t.close_glyph);
        if resp.on_hover_text("Close").clicked() {
            self.save_now();
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
        let pin = Rect::from_center_size(close.center() - Vec2::new(38.0, 0.0), Vec2::splat(28.0));
        let tint = if self.state.pinned { t.accent } else { t.icon_faint };
        let tip = if self.state.pinned { "Floating on top (click to unpin)" } else { "Float on top" };
        if icon_button(ui, pin, if self.state.pinned { "pin-filled" } else { "pin" }, 19.0, tint, t, "Float on top").on_hover_text(tip).clicked() {
            self.toggle_pin(ui.ctx(), now);
        }
    }

    fn tabs(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        let drag = ui.interact(rect, Id::new("drag-tabs"), Sense::click_and_drag());
        if drag.drag_started() && !self.in_resize_band(ui) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            self.add_note(now);
        }
        // Right side: add and more.
        let more = Rect::from_center_size(Pos2::new(rect.right() - PAD - 10.0, rect.center().y), Vec2::splat(28.0));
        let add = more.translate(Vec2::new(-34.0, 0.0));
        if icon_button(ui, add, "plus", 20.0, t.icon, t, "New note").on_hover_text("New note (Ctrl+N)").clicked() {
            self.add_note(now);
        }
        let more_resp = icon_button(ui, more, "ellipsis", 20.0, t.icon, t, "More").on_hover_text("More");
        if more_resp.clicked() {
            self.toggle_popup(Popup::More, more);
        }

        let strip = Rect::from_min_max(Pos2::new(rect.left() + PAD - 2.0, rect.top()), Pos2::new(add.left() - 6.0, rect.bottom()));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(strip).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.set_clip_rect(strip.intersect(ui.clip_rect()));
        let mut switch_to = None;
        let mut close = None;
        egui::ScrollArea::horizontal()
            .id_salt("tab-strip")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut child, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for i in 0..self.state.notes.len() {
                    match self.tab(ui, i, t, now) {
                        TabAction::None => {}
                        TabAction::Select => switch_to = Some(i),
                        TabAction::Close => close = Some(i),
                    }
                }
            });
        // Tabs that overflow fade out instead of being cut off at a hard edge.
        let fade = Rect::from_min_max(Pos2::new(strip.right() - 14.0, strip.top()), strip.right_bottom());
        let mut mesh = egui::Mesh::default();
        let bg = t.bg_top;
        mesh.colored_vertex(fade.left_top(), Color32::TRANSPARENT);
        mesh.colored_vertex(fade.right_top(), bg);
        mesh.colored_vertex(fade.left_bottom(), Color32::TRANSPARENT);
        mesh.colored_vertex(fade.right_bottom(), bg);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        ui.painter().add(mesh);
        if let Some(i) = switch_to
            && self.state.active != i
        {
            self.state.active = i;
            self.focus_editor = true;
            self.mark_dirty(now);
        }
        if let Some(i) = close {
            self.close_note(i, now);
        }
    }

    fn tab(&mut self, ui: &mut egui::Ui, i: usize, t: &Tokens, now: f64) -> TabAction {
        let selected = i == self.state.active;
        let h = 28.0;
        // Inline rename.
        if let Some((idx, buf, grab)) = &mut self.renaming
            && *idx == i
        {
            let edit = egui::TextEdit::singleline(buf)
                .font(theme::semibold(14.0))
                .desired_width(110.0)
                .margin(egui::Margin::symmetric(8, 5))
                .background_color(t.tab_selected);
            let resp = ui.add(edit);
            if std::mem::take(grab) {
                resp.request_focus();
            }
            let commit = resp.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.renaming = None;
            } else if commit {
                let title = buf.trim().to_owned();
                if !title.is_empty() {
                    self.state.notes[i].title = title;
                }
                self.renaming = None;
                self.focus_editor = true;
                self.mark_dirty(now);
            }
            return TabAction::None;
        }

        let title = self.state.notes[i].title.clone();
        let font = if selected { theme::semibold(14.0) } else { theme::regular(14.0) };
        let color = if selected { t.text } else { t.text_muted };
        let galley = ui.painter().layout_no_wrap(title, font, color);
        let x_w = if selected { 20.0 } else { 0.0 };
        let size = Vec2::new(galley.size().x + 20.0 + x_w, h);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &self.state.notes[i].title));
        if selected {
            ui.painter().rect_filled(rect, 7.0, t.tab_selected);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 7.0, t.hover);
        }
        ui.painter().galley(Pos2::new(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, color);
        let id = self.state.notes[i].id;
        if selected && self.scrolled_to != Some(id) {
            self.scrolled_to = Some(id);
            ui.scroll_to_rect(rect, None);
        }
        let mut action = TabAction::None;
        if selected {
            let x_rect = Rect::from_center_size(Pos2::new(rect.right() - 16.0, rect.center().y), Vec2::splat(18.0));
            let x = ui.interact(x_rect, Id::new(("tab-x", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close note"));
            if x.hovered() {
                ui.painter().rect_filled(x_rect, 4.0, t.hover);
            }
            icons::paint(ui, x_rect, "x", 13.0, if x.hovered() { t.text } else { t.text_muted });
            if x.on_hover_text("Close note (Ctrl+W)").clicked() {
                return TabAction::Close;
            }
        }
        if resp.double_clicked() {
            self.renaming = Some((i, self.state.notes[i].title.clone(), true));
        } else if resp.clicked() {
            action = TabAction::Select;
        } else if resp.middle_clicked() {
            action = TabAction::Close;
        }
        resp.on_hover_text("Double-click to rename");
        action
    }

    fn editor(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        let font = match self.state.font {
            FontStyle::Sans => egui::FontId::proportional(self.state.font_size),
            FontStyle::Mono => egui::FontId::monospace(self.state.font_size - 1.0),
        };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        let hidden = self.state.current().hidden;
        let focus = std::mem::take(&mut self.focus_editor);
        let placeholder = egui::RichText::new("Type anything. It saves by itself.").color(t.placeholder).font(font.clone());
        let mut changed = false;
        egui::ScrollArea::vertical().id_salt(("editor", self.state.active)).auto_shrink(false).show(&mut child, |ui| {
            let note = &mut self.state.notes[self.state.active];
            let mut mask = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                // Hidden notes keep their line structure but show only dots; the text itself is untouched.
                let shown: String = text.as_str().chars().map(|c| if c == '\n' { '\n' } else { '•' }).collect();
                let mut job = egui::text::LayoutJob::simple(shown, font.clone(), t.text, wrap);
                job.wrap.max_width = wrap;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let mut edit = egui::TextEdit::multiline(&mut note.text)
                .id(Id::new(("editor", note.id)))
                .frame(egui::Frame::NONE)
                .font(font.clone())
                .text_color(t.text)
                .hint_text(placeholder)
                .desired_width(f32::INFINITY)
                .min_size(Vec2::new(rect.width(), rect.height()))
                .margin(egui::Margin::symmetric(0, 0))
                .lock_focus(true);
            if hidden {
                edit = edit.layouter(&mut mask);
            }
            let resp = edit.show(ui).response;
            if focus {
                resp.request_focus();
            }
            changed = resp.changed();
        });
        if changed {
            self.mark_dirty(now);
        }
        if hidden && self.state.current().text.is_empty() {
            ui.painter().text(
                Pos2::new(rect.right(), rect.top()),
                Align2::RIGHT_TOP,
                "hidden",
                theme::regular(11.0),
                t.placeholder,
            );
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        let y = rect.center().y;
        let size = Vec2::splat(32.0);
        let mut x = rect.left() + PAD + 10.0;
        // "Aa" is text, not an icon glyph: it reads the same everywhere since the font is embedded.
        let aa = Rect::from_center_size(Pos2::new(x + 2.0, y), Vec2::new(34.0, 32.0));
        let resp = ui.interact(aa, Id::new("font"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Font"));
        if resp.hovered() || self.popup.is_some_and(|(p, _)| p == Popup::Font) {
            ui.painter().rect_filled(aa, 7.0, t.hover);
        }
        ui.painter().text(aa.center(), Align2::CENTER_CENTER, "Aa", theme::regular(16.0), t.text_muted);
        if resp.on_hover_text("Font").clicked() {
            self.toggle_popup(Popup::Font, aa);
        }
        x += 40.0;
        let hidden = self.state.current().hidden;
        let eye = Rect::from_center_size(Pos2::new(x, y), size);
        let (name, tip) = if hidden { ("eye-off", "Show note (Ctrl+Shift+H)") } else { ("eye", "Hide note (Ctrl+Shift+H)") };
        let tint = if hidden { t.accent } else { t.icon_faint };
        if icon_button(ui, eye, name, 20.0, tint, t, "Hide note").on_hover_text(tip).clicked() {
            self.toggle_hidden(now);
        }
        x += 38.0;
        let copy = Rect::from_center_size(Pos2::new(x, y), size);
        if icon_button(ui, copy, "files", 19.0, t.icon_faint, t, "Copy").on_hover_text("Copy all (Ctrl+Shift+C)").clicked() {
            self.copy(ui.ctx(), now);
        }
        x += 38.0;
        let save = Rect::from_center_size(Pos2::new(x, y), size);
        if icon_button(ui, save, "download", 19.0, t.icon_faint, t, "Save to file").on_hover_text("Save to file… (Ctrl+S)").clicked() {
            self.export();
        }
        let trash = Rect::from_center_size(Pos2::new(rect.right() - PAD - 10.0, y), size);
        if icon_button(ui, trash, "trash-2", 19.0, t.icon_faint, t, "Clear note").on_hover_text("Clear note").clicked() {
            self.clear(now);
        }
    }

    fn toggle_popup(&mut self, kind: Popup, anchor: Rect) {
        self.popup = match self.popup {
            Some((k, _)) if k == kind => None,
            _ => Some((kind, anchor)),
        };
    }

    fn popups(&mut self, ctx: &egui::Context, window: Rect, t: &Tokens, now: f64) {
        let Some((kind, anchor)) = self.popup else { return };
        let (pos, pivot) = match kind {
            Popup::Font => (anchor.left_top() - Vec2::new(0.0, 4.0), Align2::LEFT_BOTTOM),
            Popup::More => (anchor.right_bottom() + Vec2::new(0.0, 4.0), Align2::RIGHT_TOP),
        };
        let frame = egui::Frame::new()
            .fill(t.popup)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(8)
            .inner_margin(egui::Margin::same(5))
            .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(70) });
        let area = egui::Area::new(Id::new("popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .pivot(pivot)
            .constrain_to(window.shrink(4.0))
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.set_min_width(150.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    match kind {
                        Popup::Font => self.font_menu(ui, t, now),
                        Popup::More => self.more_menu(ui, t, now),
                    }
                });
            });
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx.pointer_interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p));
        if clicked_outside {
            self.popup = None;
        }
    }

    fn font_menu(&mut self, ui: &mut egui::Ui, t: &Tokens, now: f64) {
        for (style, label) in [(FontStyle::Sans, "Sans – Inter"), (FontStyle::Mono, "Mono – JetBrains Mono")] {
            if menu_item(ui, t, label, self.state.font == style).clicked() {
                self.state.font = style;
                self.mark_dirty(now);
            }
        }
        separator(ui, t);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(format!("Size {:.0}", self.state.font_size)).color(t.text_muted).font(theme::regular(13.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (label, delta) in [("A+", 1.0), ("A−", -1.0)] {
                    let b = egui::Button::new(egui::RichText::new(label).font(theme::semibold(13.0)).color(t.text)).frame(false);
                    if ui.add(b).clicked() {
                        self.state.font_size = (self.state.font_size + delta).clamp(10.0, 32.0);
                        self.mark_dirty(now);
                    }
                }
            });
        });
    }

    fn more_menu(&mut self, ui: &mut egui::Ui, t: &Tokens, now: f64) {
        if menu_item(ui, t, "Rename note", false).clicked() {
            self.renaming = Some((self.state.active, self.state.current().title.clone(), true));
            self.popup = None;
        }
        if menu_item(ui, t, "Close note", false).clicked() {
            self.close_note(self.state.active, now);
            self.popup = None;
        }
        separator(ui, t);
        let other = match self.state.theme {
            ThemeKind::Dark => (ThemeKind::Light, "Light theme"),
            ThemeKind::Light => (ThemeKind::Dark, "Dark theme"),
        };
        if menu_item(ui, t, other.1, false).clicked() {
            self.state.theme = other.0;
            self.mark_dirty(now);
            self.popup = None;
        }
        if menu_item(ui, t, "Float on top", self.state.pinned).clicked() {
            self.toggle_pin(ui.ctx(), now);
        }
        separator(ui, t);
        if menu_item(ui, t, "Quit", false).clicked() {
            self.save_now();
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn toast_ui(&mut self, ui: &mut egui::Ui, above: Rect, t: &Tokens, now: f64) {
        let Some(toast) = &self.toast else { return };
        if now > toast.until {
            self.toast = None;
            return;
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(toast.until - now));
        let font = theme::regular(13.0);
        let text = ui.painter().layout_no_wrap(toast.text.clone(), font.clone(), t.text);
        let undo_label = toast.undo.as_ref().map(|_| ui.painter().layout_no_wrap("Undo".into(), theme::semibold(13.0), t.accent));
        let w = text.size().x + 24.0 + undo_label.as_ref().map_or(0.0, |g| g.size().x + 14.0);
        let rect = Rect::from_center_size(Pos2::new(above.center().x, above.bottom() - 22.0), Vec2::new(w, 28.0));
        let p = ui.painter();
        p.rect_filled(rect.translate(Vec2::new(0.0, 2.0)), 14.0, Color32::from_black_alpha(40));
        p.rect_filled(rect, 14.0, t.popup);
        p.rect_stroke(rect, 14.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let text_w = text.size().x;
        p.galley(Pos2::new(rect.left() + 12.0, rect.center().y - text.size().y / 2.0), text, t.text);
        if let Some(g) = undo_label {
            let r = Rect::from_min_size(Pos2::new(rect.left() + 12.0 + text_w + 14.0, rect.center().y - g.size().y / 2.0), g.size());
            ui.painter().galley(r.min, g, t.accent);
            let resp = ui.interact(r.expand(4.0), Id::new("undo"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Undo"));
            if resp.clicked()
                && let Some(undo) = self.toast.take().and_then(|t| t.undo)
            {
                self.undo(undo, now);
            }
        }
    }

    /// Sends a pending pin state to Hyprland and picks up pinning done with its own keys.
    #[cfg(target_os = "linux")]
    fn sync_wm(&mut self, ctx: &egui::Context, now: f64) {
        let Some(wm) = &mut self.wm else { return };
        if let Some((pinned, deadline)) = wm.pending {
            // The start-up request gets its deadline once the clock runs.
            let deadline = if deadline.is_nan() { now + 5.0 } else { deadline };
            wm.pending = Some((pinned, deadline));
            if wm.hypr.set_pinned(pinned) {
                wm.pending = None;
                wm.ignore_until = now + 1.5;
                if std::mem::take(&mut wm.restore) {
                    wm.hypr.place(self.state.window_size, self.state.window_pos);
                }
            } else if now < deadline {
                // Not mapped yet right after start: try again shortly.
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            } else {
                wm.pending = None;
            }
        }
        while let Ok(win) = wm.observed.try_recv() {
            if wm.pending.is_some() || now < wm.ignore_until {
                continue;
            }
            if win.pinned != self.state.pinned {
                self.state.pinned = win.pinned;
                self.dirty_since.get_or_insert(now);
            }
            // Only a floating window's geometry is worth remembering; a tiled one is the layout's.
            if win.floating {
                let size = Some([win.size[0] as f32, win.size[1] as f32]);
                let pos = Some([win.at[0] as f32, win.at[1] as f32]);
                if (size, pos) != (self.state.window_size, self.state.window_pos) {
                    self.state.window_size = size;
                    self.state.window_pos = pos;
                    self.dirty_since.get_or_insert(now);
                }
            }
        }
    }

    /// Remembers the window's size (and position where the platform reports it) for next start.
    fn track_geometry(&mut self, ctx: &egui::Context, now: f64) {
        #[cfg(target_os = "linux")]
        if self.wm.is_some() {
            return; // Hyprland reports it (and only for floating windows): see sync_wm.
        }
        let (inner, outer, odd) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.minimized == Some(true) || v.maximized == Some(true) || v.fullscreen == Some(true))
        });
        let Some(inner) = inner.filter(|_| !odd) else { return };
        let size = Some([inner.width().round(), inner.height().round()]);
        // Wayland never reports a real position.
        let pos = outer.filter(|_| std::env::var_os("WAYLAND_DISPLAY").is_none()).map(|r| [r.min.x.round(), r.min.y.round()]);
        if size != self.state.window_size || (pos.is_some() && pos != self.state.window_pos) {
            self.state.window_size = size;
            self.state.window_pos = pos.or(self.state.window_pos);
            self.mark_dirty(now);
        }
    }

    fn poll_saves(&mut self, now: f64) {
        let Some(rx) = &self.saves else { return };
        match rx.try_recv() {
            Ok(Ok(path)) => {
                let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                self.saves = None;
                self.toast(now, format!("Saved {name}"), None);
            }
            Ok(Err(e)) => {
                self.saves = None;
                self.toast(now, format!("Save failed: {e}"), None);
            }
            Err(mpsc::TryRecvError::Disconnected) => self.saves = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Draws one frame into `ui` (the full window). Shared by eframe and the headless tests.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let t = self.tokens();
        if self.applied_theme != Some(t.kind) {
            t.apply(&ctx);
            self.applied_theme = Some(t.kind);
        }
        // Fonts given to set_fonts only exist from the next frame: named families would panic now.
        if !self.fonts_ready {
            egui_extras::install_image_loaders(&ctx);
            theme::install_fonts(&ctx);
            self.fonts_ready = true;
            ctx.request_repaint();
            return;
        }
        self.poll_saves(now);
        if self.saves.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        self.shortcuts(&ctx, now);
        #[cfg(target_os = "linux")]
        self.sync_wm(&ctx, now);
        self.track_geometry(&ctx, now);

        let window = ctx.content_rect();
        self.window_frame(ui, window, &t);
        let header = Rect::from_min_size(window.min, Vec2::new(window.width(), HEADER_H));
        let tabs = Rect::from_min_size(Pos2::new(window.left(), header.bottom()), Vec2::new(window.width(), TABS_H));
        let footer = Rect::from_min_max(Pos2::new(window.left(), window.bottom() - FOOTER_H), window.max);
        let divider_y = tabs.bottom() + 3.0;
        ui.painter().hline(window.x_range().shrink(1.0), divider_y, Stroke::new(1.0, t.divider));
        let editor = Rect::from_min_max(Pos2::new(window.left() + PAD + 4.0, divider_y + 10.0), Pos2::new(window.right() - PAD, footer.top()));

        self.header(ui, header, &t, now);
        self.tabs(ui, tabs, &t, now);
        self.editor(ui, editor, &t, now);
        self.footer(ui, footer, &t, now);
        self.toast_ui(ui, Rect::from_min_max(editor.min, Pos2::new(window.right(), footer.top())), &t, now);
        self.popups(&ctx, window, &t, now);
        self.resize_edges(ui, window);

        if let Some(since) = self.dirty_since {
            if now - since >= SAVE_DELAY {
                self.save_now();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(SAVE_DELAY));
            }
        }
    }
}

enum TabAction {
    None,
    Select,
    Close,
}

/// A square icon button: transparent until hovered.
fn icon_button(ui: &mut egui::Ui, rect: Rect, icon: &str, size: f32, tint: Color32, t: &Tokens, label: &str) -> Response {
    let resp = ui.interact(rect, Id::new(("icon-btn", label)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if resp.hovered() {
        ui.painter().rect_filled(rect, 7.0, t.hover);
    }
    let shift = if resp.is_pointer_button_down_on() { Vec2::new(0.0, 1.0) } else { Vec2::ZERO };
    icons::paint(ui, rect.translate(shift), icon, size, tint);
    resp
}

fn menu_item(ui: &mut egui::Ui, t: &Tokens, label: &str, checked: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(150.0), 26.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, checked, label));
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, t.hover);
    }
    ui.painter().text(Pos2::new(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, label, theme::regular(13.5), t.text);
    if checked {
        ui.painter().circle_filled(Pos2::new(rect.right() - 12.0, rect.center().y), 3.5, t.accent);
    }
    resp
}

fn separator(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 7.0), Sense::hover());
    ui.painter().hline(rect.x_range().shrink(4.0), rect.center().y, Stroke::new(1.0, t.divider));
}

/// A file name from a note title: no path separators or reserved characters.
fn sanitize(title: &str) -> String {
    let s: String = title.chars().map(|c| if r#"/\:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    if s.trim().is_empty() { "note".into() } else { s.trim().to_owned() }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_now();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if self.dirty_since.is_some() {
            self.save_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize("API keys / prod"), "API keys _ prod");
        assert_eq!(sanitize("  "), "note");
    }
}
