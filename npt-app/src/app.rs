//! Stato dell'applicazione e pannelli dell'interfaccia.

use crate::icons::{self, ICON_SIZE};
use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2,
};
use npt_core::addr::{default_prefix_for, mask_to_prefix};
use npt_core::device::{short_iface_name, RouteKind};
use npt_core::sim::{validate_host_ip, EventKind};
use npt_core::{DeviceId, DeviceKind, Endpoint, Ipv4Cidr, LinkId, Network, PingStatus, StaticRoute};
use std::net::Ipv4Addr;
use std::path::PathBuf;

const CANVAS_BG: Color32 = Color32::from_rgb(244, 246, 249);
const GRID: Color32 = Color32::from_rgb(228, 232, 238);
const LINK_UP: Color32 = Color32::from_rgb(40, 170, 80);
const LINK_DOWN: Color32 = Color32::from_rgb(210, 60, 50);
const SELECT: Color32 = Color32::from_rgb(255, 160, 0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Place(DeviceKind),
    Cable,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Realtime,
    Simulation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Device(DeviceId),
    Link(LinkId),
}

struct IfaceEdit {
    ip: String,
    mask: String,
}

/// Campi di testo del pannello proprietà (applicati con "Applica").
struct Editor {
    device: DeviceId,
    name: String,
    ifaces: Vec<IfaceEdit>,
    gateway: String,
    route_net: String,
    route_mask: String,
    route_nh: String,
    message: Option<(bool, String)>,
}

struct PortDialog {
    a: DeviceId,
    b: DeviceId,
    ia: usize,
    ib: usize,
}

pub struct NptApp {
    net: Network,
    tool: Tool,
    mode: Mode,
    selection: Option<Selection>,
    cable_from: Option<DeviceId>,
    port_dialog: Option<PortDialog>,
    pan: Vec2,
    editor: Option<Editor>,
    ping_src: Option<DeviceId>,
    ping_dst: String,
    sim_playing: bool,
    sim_progress: f32,
    sim_speed: f32,
    selected_event: Option<usize>,
    file: Option<PathBuf>,
    dirty: bool,
    status: String,
    last_title: String,
    show_about: bool,
}

impl NptApp {
    pub fn new() -> Self {
        Self {
            net: Network::new(),
            tool: Tool::Select,
            mode: Mode::Realtime,
            selection: None,
            cable_from: None,
            port_dialog: None,
            pan: Vec2::ZERO,
            editor: None,
            ping_src: None,
            ping_dst: String::new(),
            sim_playing: false,
            sim_progress: 0.0,
            sim_speed: 1.5,
            selected_event: None,
            file: None,
            dirty: false,
            status: "Pronto. Scegli un dispositivo a sinistra e clicca sull'area di lavoro.".into(),
            last_title: String::new(),
            show_about: false,
        }
    }

    pub fn open_path(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| Network::from_json(&s)) {
            Ok(net) => {
                self.net = net;
                self.status = format!("Aperto {}", path.display());
                self.file = Some(path);
                self.dirty = false;
                self.selection = None;
                self.editor = None;
                self.ping_src = None;
            }
            Err(e) => self.status = format!("Errore apertura: {e}"),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.handle_keys(&ctx);
        self.top_bar(ui);
        self.bottom_panel(ui);
        self.left_panel(ui);
        self.right_panel(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(CANVAS_BG))
            .show(ui, |ui| self.canvas(ui));
        self.port_dialog_window(&ctx);
        self.about_window(&ctx);
        self.sim_animate(&ctx);
        self.update_title(&ctx);
    }

    // ------------------------------------------------------------------
    // File
    // ------------------------------------------------------------------

    fn new_file(&mut self) {
        *self = Self { status: "Nuovo progetto".into(), ..Self::new() };
    }

    fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("NetPackeT", &["npt"]).pick_file() {
            self.open_path(p);
        }
    }

    fn save(&mut self, save_as: bool) {
        let path = match (&self.file, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new()
                .add_filter("NetPackeT", &["npt"])
                .set_file_name("rete.npt")
                .save_file()
                .map(|p| if p.extension().is_none() { p.with_extension("npt") } else { p }),
        };
        let Some(path) = path else { return };
        match self.net.to_json().and_then(|j| std::fs::write(&path, j).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.status = format!("Salvato {}", path.display());
                self.file = Some(path);
                self.dirty = false;
            }
            Err(e) => self.status = format!("Errore salvataggio: {e}"),
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self
            .file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Senza titolo".into());
        let title = format!("{}{name} — NPT easy", if self.dirty { "• " } else { "" });
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    // ------------------------------------------------------------------
    // Tastiera
    // ------------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (del, esc, save, open, new) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::Escape),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::N),
            )
        });
        if del {
            match self.selection.take() {
                Some(Selection::Device(d)) => self.delete_device(d),
                Some(Selection::Link(l)) => self.delete_link(l),
                None => {}
            }
        }
        if esc {
            self.tool = Tool::Select;
            self.cable_from = None;
        }
        if save {
            self.save(false);
        }
        if open {
            self.open_dialog();
        }
        if new {
            self.new_file();
        }
    }

    fn delete_device(&mut self, d: DeviceId) {
        let name = self.net.device_name(d);
        self.net.remove_device(d);
        if self.editor.as_ref().is_some_and(|e| e.device == d) {
            self.editor = None;
        }
        if self.ping_src == Some(d) {
            self.ping_src = None;
        }
        self.dirty = true;
        self.status = format!("Eliminato {name}");
    }

    fn delete_link(&mut self, l: LinkId) {
        self.net.disconnect(l);
        self.dirty = true;
        self.status = "Cavo rimosso".into();
    }

    // ------------------------------------------------------------------
    // Barra superiore
    // ------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.label(RichText::new("NPT").strong().size(18.0).color(Color32::from_rgb(55, 110, 190)));
                ui.label(RichText::new("easy").italics().size(13.0));
                ui.separator();
                ui.menu_button("File", |ui| {
                    if ui.button("Nuovo        Ctrl+N").clicked() {
                        self.new_file();
                    }
                    if ui.button("Apri…        Ctrl+O").clicked() {
                        self.open_dialog();
                    }
                    if ui.button("Salva        Ctrl+S").clicked() {
                        self.save(false);
                    }
                    if ui.button("Salva con nome…").clicked() {
                        self.save(true);
                    }
                    ui.separator();
                    if ui.button("Esci").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Aiuto", |ui| {
                    if ui.button("Informazioni su NPT easy").clicked() {
                        self.show_about = true;
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let sim = ui.selectable_label(self.mode == Mode::Simulation, "⏱ Simulazione");
                    let rt = ui.selectable_label(self.mode == Mode::Realtime, "⚡ Tempo reale");
                    if sim.clicked() && self.mode != Mode::Simulation {
                        self.mode = Mode::Simulation;
                        self.net.events.clear();
                        self.selected_event = None;
                        self.status = "Modalità simulazione: invia un ping e usa ▶ / Avanti.".into();
                    }
                    if rt.clicked() && self.mode != Mode::Realtime {
                        self.mode = Mode::Realtime;
                        self.sim_playing = false;
                        self.net.run_until_idle(10_000);
                        self.status = "Modalità tempo reale".into();
                    }
                });
            });
        });
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("Informazioni").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("NetPackeT — NPT easy").strong().size(18.0));
            ui.label(format!("Versione {}", env!("CARGO_PKG_VERSION")));
            ui.label("Simulatore di reti didattico, nativo per Linux.");
        });
        self.show_about = open;
    }

    // ------------------------------------------------------------------
    // Palette strumenti
    // ------------------------------------------------------------------

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("tools").resizable(false).exact_size(130.0).show(ui, |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Strumenti").strong());
            for (tool, label) in [(Tool::Select, "🖱 Seleziona"), (Tool::Cable, "🔌 Cavo"), (Tool::Delete, "🗑 Elimina")] {
                if ui.add_sized([118.0, 24.0], egui::Button::selectable(self.tool == tool, label)).clicked() {
                    self.tool = tool;
                    self.cable_from = None;
                }
            }
            ui.separator();
            ui.label(RichText::new("Dispositivi").strong());
            for kind in DeviceKind::ALL {
                let selected = self.tool == Tool::Place(kind);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(118.0, 56.0), Sense::click());
                let bg = if selected {
                    Color32::from_rgb(255, 236, 200)
                } else if resp.hovered() {
                    Color32::from_gray(235)
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 4.0, bg);
                icons::draw_device(ui.painter(), rect.center() - Vec2::new(0.0, 7.0), kind, 0.6);
                ui.painter().text(
                    rect.center_bottom() - Vec2::new(0.0, 3.0),
                    Align2::CENTER_BOTTOM,
                    kind.label(),
                    FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
                if resp.clicked() {
                    self.tool = if selected { Tool::Select } else { Tool::Place(kind) };
                    self.cable_from = None;
                    self.status = format!("Clicca sull'area di lavoro per aggiungere un {}", kind.label());
                }
            }
        });
    }

    // ------------------------------------------------------------------
    // Area di lavoro
    // ------------------------------------------------------------------

    fn to_screen(&self, origin: Pos2, p: [f32; 2]) -> Pos2 {
        origin + self.pan + Vec2::new(p[0], p[1])
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let bg = ui.interact(rect, ui.scope_id().with("canvas-bg"), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let origin = rect.min;

        // Griglia di sfondo.
        let step = 40.0;
        let off = Vec2::new(self.pan.x.rem_euclid(step), self.pan.y.rem_euclid(step));
        let mut x = rect.left() + off.x;
        while x < rect.right() {
            painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0, GRID));
            x += step;
        }
        let mut y = rect.top() + off.y;
        while y < rect.bottom() {
            painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0, GRID));
            y += step;
        }

        // Cavi.
        let mut link_segments: Vec<(LinkId, Pos2, Pos2)> = Vec::new();
        for link in self.net.links.values() {
            let (Some(da), Some(db)) = (self.net.device(link.a.device), self.net.device(link.b.device)) else {
                continue;
            };
            let pa = self.to_screen(origin, da.pos);
            let pb = self.to_screen(origin, db.pos);
            link_segments.push((link.id, pa, pb));
            let selected = self.selection == Some(Selection::Link(link.id));
            let color = if selected { SELECT } else { Color32::from_rgb(60, 60, 70) };
            let stroke = Stroke::new(if selected { 3.5 } else { 2.0 }, color);
            if link.cable == npt_core::sim::CableKind::CopperCross {
                painter.extend(Shape::dashed_line(&[pa, pb], stroke, 8.0, 5.0));
            } else {
                painter.line_segment([pa, pb], stroke);
            }
            // Luci di stato alle estremità e nome della porta.
            for (ep, from, to) in [(link.a, pa, pb), (link.b, pb, pa)] {
                let dev = self.net.device(ep.device).unwrap();
                let ifc = &dev.interfaces[ep.iface];
                let dir = (to - from).normalized();
                // Distanza a cui il cavo esce dall'ingombro del dispositivo
                // (sotto l'icona c'è anche l'etichetta con nome e IP).
                let half_w = ICON_SIZE.x / 2.0 + 6.0;
                let half_h = if dir.y > 0.0 { ICON_SIZE.y / 2.0 + 38.0 } else { ICON_SIZE.y / 2.0 + 6.0 };
                let exit = (half_w / dir.x.abs().max(1e-3)).min(half_h / dir.y.abs().max(1e-3));
                let length = (to - from).length();
                let d_light = (exit + 6.0).min(length * 0.35);
                let light = from + dir * d_light;
                let up = self.net.link_is_up(link);
                painter.circle_filled(light, 5.0, if up { LINK_UP } else { LINK_DOWN });
                let perp = Vec2::new(-dir.y, dir.x);
                let perp = if perp.y > 0.0 { -perp } else { perp };
                painter.text(
                    light + dir * 14.0 + perp * 11.0,
                    Align2::CENTER_CENTER,
                    short_iface_name(&ifc.name),
                    FontId::proportional(10.0),
                    Color32::from_gray(90),
                );
            }
        }

        // Pacchetti in transito (solo in simulazione).
        if self.mode == Mode::Simulation {
            for f in &self.net.in_flight {
                let (Some(a), Some(b)) = (self.net.device(f.from.device), self.net.device(f.to.device)) else {
                    continue;
                };
                let pa = self.to_screen(origin, a.pos);
                let pb = self.to_screen(origin, b.pos);
                let t = self.sim_progress.clamp(0.0, 1.0);
                let p = pa + (pb - pa) * t;
                let color = match f.frame.protocol() {
                    "ARP" => Color32::from_rgb(240, 190, 30),
                    _ => Color32::from_rgb(150, 80, 200),
                };
                let env = Rect::from_center_size(p, Vec2::new(22.0, 15.0));
                painter.rect_filled(env, 2.0, color);
                painter.rect_stroke(env, 2.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Middle);
                painter.line_segment([env.left_top(), env.center()], Stroke::new(1.0, Color32::BLACK));
                painter.line_segment([env.right_top(), env.center()], Stroke::new(1.0, Color32::BLACK));
            }
        }

        // Dispositivi.
        let ids: Vec<DeviceId> = self.net.devices.keys().copied().collect();
        let mut clicked_device = None;
        for id in ids {
            let dev = self.net.device(id).unwrap();
            let center = self.to_screen(origin, dev.pos);
            let hit = Rect::from_center_size(center, ICON_SIZE + Vec2::new(8.0, 8.0));
            let resp = ui.interact(hit, ui.scope_id().with(("device", id)), Sense::click_and_drag());

            let selected = self.selection == Some(Selection::Device(id)) || self.cable_from == Some(id);
            if selected || resp.hovered() {
                let c = if selected { SELECT } else { Color32::from_gray(180) };
                painter.rect_stroke(hit, 6.0, Stroke::new(2.0, c), egui::StrokeKind::Middle);
            }
            icons::draw_device(&painter, center, dev.kind, 1.0);
            painter.text(
                hit.center_bottom() + Vec2::new(0.0, 2.0),
                Align2::CENTER_TOP,
                &dev.name,
                FontId::proportional(13.0),
                Color32::from_gray(20),
            );
            if let Some(ip) = dev.primary_ip() {
                painter.text(
                    hit.center_bottom() + Vec2::new(0.0, 17.0),
                    Align2::CENTER_TOP,
                    ip.to_string(),
                    FontId::proportional(10.5),
                    Color32::from_gray(90),
                );
            }

            if resp.dragged() && self.tool == Tool::Select {
                let d = resp.drag_delta();
                let dev = self.net.device_mut(id).unwrap();
                dev.pos[0] += d.x;
                dev.pos[1] += d.y;
                self.dirty = true;
            }
            if resp.clicked() {
                clicked_device = Some(id);
            }
            if self.tool == Tool::Select {
                let tip = self.device_tooltip(id);
                resp.on_hover_text(tip);
            }
        }

        if let Some(id) = clicked_device {
            self.on_device_click(id);
        }

        // Anteprima del cavo in fase di posa.
        if let (Some(from), Some(pointer)) = (self.cable_from, ui.ctx().pointer_hover_pos()) {
            if let Some(d) = self.net.device(from) {
                let p = self.to_screen(origin, d.pos);
                painter.extend(Shape::dashed_line(&[p, pointer], Stroke::new(2.0, SELECT), 6.0, 4.0));
            }
        }

        // Interazioni con lo sfondo.
        if bg.dragged() && matches!(self.tool, Tool::Select) || bg.dragged_by(egui::PointerButton::Middle) {
            self.pan += bg.drag_delta();
        }
        if bg.clicked() {
            if let Some(pos) = bg.interact_pointer_pos() {
                let world = pos - origin - self.pan;
                let link_hit = link_segments
                    .iter()
                    .find(|(_, a, b)| dist_to_segment(pos, *a, *b) < 6.0)
                    .map(|(id, ..)| *id);
                match self.tool {
                    Tool::Place(kind) => {
                        let id = self.net.add_device(kind, [world.x, world.y]);
                        self.dirty = true;
                        self.selection = Some(Selection::Device(id));
                        self.load_editor(id);
                        self.status = format!("Aggiunto {}", self.net.device_name(id));
                    }
                    Tool::Delete => {
                        if let Some(l) = link_hit {
                            self.delete_link(l);
                        }
                    }
                    Tool::Cable => {
                        self.cable_from = None;
                    }
                    Tool::Select => {
                        self.selection = link_hit.map(Selection::Link);
                        if link_hit.is_none() {
                            self.editor = None;
                        }
                    }
                }
            }
        }

        if self.net.devices.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Scegli un dispositivo a sinistra e clicca qui per aggiungerlo.\nPoi usa lo strumento 🔌 Cavo per collegarli.",
                FontId::proportional(16.0),
                Color32::from_gray(140),
            );
        }
    }

    fn device_tooltip(&self, id: DeviceId) -> String {
        let dev = self.net.device(id).unwrap();
        let mut s = format!("{} ({})", dev.name, dev.kind.label());
        for (i, ifc) in dev.interfaces.iter().enumerate() {
            if ifc.link.is_none() && ifc.ip.is_none() {
                continue;
            }
            let peer = self
                .net
                .peer_of(Endpoint { device: id, iface: i })
                .map(|(d, p)| self.net.endpoint_label(Endpoint { device: d, iface: p }))
                .unwrap_or_else(|| "—".into());
            let ip = ifc.ip.map(|c| c.to_string()).unwrap_or_default();
            let state = if ifc.enabled { "" } else { " [shutdown]" };
            s.push_str(&format!("\n{} {ip}{state} ↔ {peer}", short_iface_name(&ifc.name)));
        }
        if let Some(gw) = dev.gateway {
            s.push_str(&format!("\nGateway: {gw}"));
        }
        s
    }

    fn on_device_click(&mut self, id: DeviceId) {
        match self.tool {
            Tool::Select | Tool::Place(_) => {
                self.selection = Some(Selection::Device(id));
                self.load_editor(id);
                if self.ping_src.is_none() && !self.net.device(id).unwrap().kind.is_l2() {
                    self.ping_src = Some(id);
                }
            }
            Tool::Delete => self.delete_device(id),
            Tool::Cable => match self.cable_from {
                None => {
                    if self.net.device(id).unwrap().first_free_iface().is_none() {
                        self.status = format!("{} non ha porte libere", self.net.device_name(id));
                    } else {
                        self.cable_from = Some(id);
                        self.status = format!("Cavo da {}: clicca il secondo dispositivo", self.net.device_name(id));
                    }
                }
                Some(a) if a == id => self.cable_from = None,
                Some(a) => {
                    self.cable_from = None;
                    let ia = self.net.device(a).unwrap().first_free_iface();
                    let ib = self.net.device(id).unwrap().first_free_iface();
                    match (ia, ib) {
                        (Some(ia), Some(ib)) => self.port_dialog = Some(PortDialog { a, b: id, ia, ib }),
                        _ => self.status = format!("{} non ha porte libere", self.net.device_name(id)),
                    }
                }
            },
        }
    }

    fn port_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.port_dialog else { return };
        let mut action: Option<bool> = None;
        egui::Window::new("Scegli le porte")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Grid::new("ports").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    for (dev_id, sel) in [(dlg.a, &mut dlg.ia), (dlg.b, &mut dlg.ib)] {
                        let dev = self.net.device(dev_id).unwrap();
                        ui.label(RichText::new(&dev.name).strong());
                        egui::ComboBox::from_id_salt(("port", dev_id))
                            .selected_text(dev.interfaces[*sel].name.clone())
                            .show_ui(ui, |ui| {
                                for (i, name) in npt_core::device::free_ports(dev) {
                                    ui.selectable_value(sel, i, name);
                                }
                            });
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Collega").clicked() {
                        action = Some(true);
                    }
                    if ui.button("Annulla").clicked() {
                        action = Some(false);
                    }
                });
            });
        match action {
            Some(true) => {
                let a = Endpoint { device: dlg.a, iface: dlg.ia };
                let b = Endpoint { device: dlg.b, iface: dlg.ib };
                match self.net.connect(a, b) {
                    Ok(_) => {
                        self.status = format!("Collegato {} ↔ {}", self.net.endpoint_label(a), self.net.endpoint_label(b));
                        self.dirty = true;
                        for d in [a.device, b.device] {
                            if self.editor.as_ref().is_some_and(|e| e.device == d) {
                                self.load_editor(d);
                            }
                        }
                    }
                    Err(e) => self.status = format!("Errore: {e}"),
                }
                self.port_dialog = None;
            }
            Some(false) => self.port_dialog = None,
            None => {}
        }
    }

    // ------------------------------------------------------------------
    // Pannello proprietà
    // ------------------------------------------------------------------

    fn load_editor(&mut self, id: DeviceId) {
        let Some(dev) = self.net.device(id) else { return };
        self.editor = Some(Editor {
            device: id,
            name: dev.name.clone(),
            ifaces: dev
                .interfaces
                .iter()
                .map(|i| IfaceEdit {
                    ip: i.ip.map(|c| c.addr.to_string()).unwrap_or_default(),
                    mask: i.ip.map(|c| c.mask().to_string()).unwrap_or_default(),
                })
                .collect(),
            gateway: dev.gateway.map(|g| g.to_string()).unwrap_or_default(),
            route_net: String::new(),
            route_mask: String::new(),
            route_nh: String::new(),
            message: None,
        });
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("props").resizable(true).default_size(330.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                match self.selection {
                    Some(Selection::Device(id)) if self.net.device(id).is_some() => self.device_editor(ui, id),
                    Some(Selection::Link(l)) if self.net.links.contains_key(&l) => self.link_info(ui, l),
                    _ => {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Proprietà").strong().size(16.0));
                        ui.label("Seleziona un dispositivo o un cavo per vederne la configurazione.");
                    }
                }
            });
        });
    }

    fn link_info(&mut self, ui: &mut egui::Ui, l: LinkId) {
        let link = self.net.links[&l].clone();
        ui.add_space(8.0);
        ui.label(RichText::new("Cavo").strong().size(16.0));
        ui.label(format!("Tipo: {}", link.cable.label()));
        ui.label(format!("Da: {}", self.net.endpoint_label(link.a)));
        ui.label(format!("A:  {}", self.net.endpoint_label(link.b)));
        let up = self.net.link_is_up(&link);
        ui.label(RichText::new(if up { "Stato: attivo" } else { "Stato: inattivo (porta spenta)" }).color(if up {
            LINK_UP
        } else {
            LINK_DOWN
        }));
        ui.add_space(8.0);
        if ui.button("🗑 Rimuovi cavo").clicked() {
            self.delete_link(l);
            self.selection = None;
        }
    }

    fn device_editor(&mut self, ui: &mut egui::Ui, id: DeviceId) {
        if self.editor.as_ref().map(|e| e.device) != Some(id) {
            self.load_editor(id);
        }
        let kind = self.net.device(id).unwrap().kind;
        let mut apply = false;
        let mut reload = false;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(44.0, 34.0), Sense::hover());
            icons::draw_device(ui.painter(), r.center(), kind, 0.62);
            ui.label(RichText::new(kind.label()).strong().size(16.0));
        });

        let ed = self.editor.as_mut().unwrap();
        ui.horizontal(|ui| {
            ui.label("Nome:");
            ui.text_edit_singleline(&mut ed.name);
        });

        // --- Interfacce ---
        ui.add_space(6.0);
        ui.label(RichText::new("Interfacce").strong());
        let dev = self.net.device(id).unwrap().clone();
        let mut toggles: Vec<(usize, bool)> = Vec::new();
        if kind.is_l2() {
            egui::Grid::new("l2ports").striped(true).num_columns(3).show(ui, |ui| {
                ui.label(RichText::new("Porta").small());
                ui.label(RichText::new("Accesa").small());
                ui.label(RichText::new("Collegata a").small());
                ui.end_row();
                for (i, ifc) in dev.interfaces.iter().enumerate() {
                    ui.label(short_iface_name(&ifc.name));
                    let mut en = ifc.enabled;
                    if ui.checkbox(&mut en, "").changed() {
                        toggles.push((i, en));
                    }
                    let peer = self
                        .net
                        .peer_of(Endpoint { device: id, iface: i })
                        .map(|(d, p)| self.net.endpoint_label(Endpoint { device: d, iface: p }))
                        .unwrap_or_else(|| "—".into());
                    ui.label(peer);
                    ui.end_row();
                }
            });
        } else {
            for (i, ifc) in dev.interfaces.iter().enumerate() {
                let peer = self
                    .net
                    .peer_of(Endpoint { device: id, iface: i })
                    .map(|(d, p)| self.net.endpoint_label(Endpoint { device: d, iface: p }))
                    .unwrap_or_else(|| "non collegata".into());
                egui::CollapsingHeader::new(format!("{}  ({peer})", ifc.name))
                    .id_salt(("iface", id, i))
                    .default_open(true)
                    .show(ui, |ui| {
                        let ie = &mut ed.ifaces[i];
                        egui::Grid::new(("ifgrid", id, i)).num_columns(2).show(ui, |ui| {
                            ui.label("Accesa:");
                            let mut en = ifc.enabled;
                            if ui.checkbox(&mut en, if kind.is_router() { "no shutdown" } else { "" }).changed() {
                                toggles.push((i, en));
                            }
                            ui.end_row();
                            ui.label("Indirizzo IP:");
                            ui.add(egui::TextEdit::singleline(&mut ie.ip).hint_text("es. 192.168.1.10"));
                            ui.end_row();
                            ui.label("Subnet mask:");
                            ui.add(egui::TextEdit::singleline(&mut ie.mask).hint_text("es. 255.255.255.0"));
                            ui.end_row();
                            ui.label("MAC:");
                            ui.label(RichText::new(ifc.mac.to_string()).monospace());
                            ui.end_row();
                        });
                    });
            }
        }
        if kind.is_host() {
            ui.horizontal(|ui| {
                ui.label("Default gateway:");
                ui.add(egui::TextEdit::singleline(&mut ed.gateway).hint_text("es. 192.168.1.1"));
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("✔ Applica").clicked() {
                apply = true;
            }
            if ui.button("↺ Annulla modifiche").clicked() {
                reload = true;
            }
        });
        if let Some((ok, msg)) = &ed.message {
            ui.colored_label(if *ok { LINK_UP } else { LINK_DOWN }, msg);
        }

        // --- Rotte statiche ---
        let mut remove_route = None;
        let mut add_route = false;
        if kind.is_router() {
            ui.add_space(8.0);
            ui.label(RichText::new("Rotte statiche").strong());
            for (i, r) in dev.static_routes.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} via {}", r.network, r.next_hop)).monospace());
                    if ui.small_button("✖").clicked() {
                        remove_route = Some(i);
                    }
                });
            }
            egui::Grid::new(("routeadd", id)).num_columns(2).show(ui, |ui| {
                ui.label("Rete:");
                ui.add(egui::TextEdit::singleline(&mut ed.route_net).hint_text("192.168.2.0"));
                ui.end_row();
                ui.label("Mask:");
                ui.add(egui::TextEdit::singleline(&mut ed.route_mask).hint_text("255.255.255.0"));
                ui.end_row();
                ui.label("Next hop:");
                ui.add(egui::TextEdit::singleline(&mut ed.route_nh).hint_text("10.0.0.2"));
                ui.end_row();
            });
            if ui.button("➕ Aggiungi rotta").clicked() {
                add_route = true;
            }
        }

        // --- Tabelle di runtime ---
        ui.add_space(8.0);
        if !kind.is_l2() {
            egui::CollapsingHeader::new("Tabella di routing").id_salt(("rt", id)).show(ui, |ui| {
                let table = dev.routing_table();
                if table.is_empty() {
                    ui.label("vuota");
                }
                for r in table {
                    let code = match r.kind {
                        RouteKind::Connected => "C",
                        RouteKind::Static => "S",
                        RouteKind::Default => "S*",
                    };
                    let via = r.next_hop.map(|n| format!("via {n}")).unwrap_or_else(|| "connessa".into());
                    let ifn = short_iface_name(&dev.interfaces[r.iface].name);
                    ui.label(RichText::new(format!("{code:<3}{} {via}, {ifn}", r.network)).monospace());
                }
            });
            egui::CollapsingHeader::new("Tabella ARP").id_salt(("arp", id)).show(ui, |ui| {
                if dev.state.arp_table.is_empty() {
                    ui.label("vuota");
                }
                for (ip, mac) in &dev.state.arp_table {
                    ui.label(RichText::new(format!("{ip:<16}{mac}")).monospace());
                }
            });
        }
        if kind == DeviceKind::Switch {
            egui::CollapsingHeader::new("Tabella MAC").id_salt(("mac", id)).default_open(true).show(ui, |ui| {
                if dev.state.mac_table.is_empty() {
                    ui.label("vuota");
                }
                for (mac, port) in &dev.state.mac_table {
                    ui.label(
                        RichText::new(format!("{mac}  {}", short_iface_name(&dev.interfaces[*port].name))).monospace(),
                    );
                }
            });
        }

        // --- Applica le azioni raccolte ---
        for (i, en) in toggles {
            self.net.device_mut(id).unwrap().interfaces[i].enabled = en;
            self.dirty = true;
        }
        if let Some(i) = remove_route {
            self.net.device_mut(id).unwrap().static_routes.remove(i);
            self.dirty = true;
        }
        if add_route {
            self.add_static_route(id);
        }
        if reload {
            self.load_editor(id);
        }
        if apply {
            self.apply_editor(id);
        }
    }

    fn add_static_route(&mut self, id: DeviceId) {
        let ed = self.editor.as_mut().unwrap();
        let result = (|| -> Result<StaticRoute, String> {
            let net: Ipv4Addr = ed.route_net.trim().parse().map_err(|_| "rete non valida".to_string())?;
            let prefix = parse_mask(&ed.route_mask, net)?;
            let next_hop: Ipv4Addr = ed.route_nh.trim().parse().map_err(|_| "next hop non valido".to_string())?;
            Ok(StaticRoute { network: Ipv4Cidr::new(net, prefix).as_network(), next_hop })
        })();
        match result {
            Ok(r) => {
                self.net.device_mut(id).unwrap().static_routes.push(r);
                ed.route_net.clear();
                ed.route_mask.clear();
                ed.route_nh.clear();
                ed.message = Some((true, format!("Rotta {} via {} aggiunta", r.network, r.next_hop)));
                self.dirty = true;
            }
            Err(e) => ed.message = Some((false, e)),
        }
    }

    fn apply_editor(&mut self, id: DeviceId) {
        let ed = self.editor.as_ref().unwrap();
        let result = (|| -> Result<(), String> {
            let name = ed.name.trim().to_string();
            if name.is_empty() {
                return Err("il nome non può essere vuoto".into());
            }
            if self.net.devices.values().any(|d| d.id != id && d.name.eq_ignore_ascii_case(&name)) {
                return Err(format!("esiste già un dispositivo chiamato {name}"));
            }
            let kind = self.net.device(id).unwrap().kind;
            let mut ips = Vec::new();
            if !kind.is_l2() {
                for (i, ie) in ed.ifaces.iter().enumerate() {
                    let ip = ie.ip.trim();
                    if ip.is_empty() {
                        ips.push(None);
                        continue;
                    }
                    let addr: Ipv4Addr = ip.parse().map_err(|_| format!("IP non valido: {ip}"))?;
                    let cidr = Ipv4Cidr::new(addr, parse_mask(&ie.mask, addr)?);
                    validate_host_ip(cidr)?;
                    let dup = self.net.devices.values().flat_map(|d| {
                        d.interfaces.iter().enumerate().map(move |(j, x)| (d, j, x))
                    }).find(|(d, j, x)| x.ip.map(|c| c.addr) == Some(addr) && !(d.id == id && *j == i));
                    if let Some((d, _, _)) = dup {
                        return Err(format!("{addr} è già usato da {}", d.name));
                    }
                    ips.push(Some(cidr));
                }
                // Due interfacce dello stesso router non possono stare nella stessa subnet.
                let set: Vec<Ipv4Cidr> = ips.iter().flatten().copied().collect();
                for (a, x) in set.iter().enumerate() {
                    for y in set.iter().skip(a + 1) {
                        if x.contains(y.addr) || y.contains(x.addr) {
                            return Err(format!("{x} e {y} sono nella stessa rete"));
                        }
                    }
                }
            }
            let gateway = if kind.is_host() && !ed.gateway.trim().is_empty() {
                Some(ed.gateway.trim().parse::<Ipv4Addr>().map_err(|_| "gateway non valido".to_string())?)
            } else {
                None
            };
            let dev = self.net.device_mut(id).unwrap();
            dev.name = name;
            if !kind.is_l2() {
                for (ifc, ip) in dev.interfaces.iter_mut().zip(ips) {
                    ifc.ip = ip;
                }
            }
            dev.gateway = gateway;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.dirty = true;
                self.load_editor(id);
                let ed = self.editor.as_mut().unwrap();
                ed.message = Some((true, "Configurazione applicata".into()));
                // Avvisa se il gateway non è nella subnet.
                let dev = self.net.device(id).unwrap();
                if let (Some(gw), Some(ip)) = (dev.gateway, dev.primary_ip()) {
                    if !ip.contains(gw) {
                        ed.message = Some((false, format!("Attenzione: il gateway {gw} non è nella rete {}", ip.as_network())));
                    }
                }
            }
            Err(e) => self.editor.as_mut().unwrap().message = Some((false, e)),
        }
    }

    // ------------------------------------------------------------------
    // Pannello inferiore: ping, eventi, ispettore PDU
    // ------------------------------------------------------------------

    fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").exact_size(24.0).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!(
                        "{} dispositivi · {} cavi · tick {}",
                        self.net.devices.len(),
                        self.net.links.len(),
                        self.net.tick
                    ));
                });
            });
        });
        egui::Panel::bottom("bottom").resizable(true).default_size(230.0).show(ui, |ui| {
            ui.add_space(4.0);
            ui.columns(2, |cols| {
                self.ping_ui(&mut cols[0]);
                if self.mode == Mode::Simulation {
                    self.sim_ui(&mut cols[1]);
                } else {
                    self.ping_results(&mut cols[1]);
                }
            });
        });
    }

    fn ping_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Ping").strong());
        let l3: Vec<(DeviceId, String)> =
            self.net.devices.values().filter(|d| !d.kind.is_l2()).map(|d| (d.id, d.name.clone())).collect();
        if self.ping_src.is_some_and(|s| self.net.device(s).is_none()) {
            self.ping_src = None;
        }
        let mut go = 0;
        ui.horizontal(|ui| {
            ui.label("Da:");
            let sel = self.ping_src.map(|s| self.net.device_name(s)).unwrap_or_else(|| "—".into());
            egui::ComboBox::from_id_salt("ping-src").selected_text(sel).show_ui(ui, |ui| {
                for (id, name) in &l3 {
                    ui.selectable_value(&mut self.ping_src, Some(*id), name);
                }
            });
            ui.label("a:");
            let resp = ui.add(egui::TextEdit::singleline(&mut self.ping_dst).hint_text("IP destinazione").desired_width(130.0));
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = if self.mode == Mode::Realtime { 4 } else { 1 };
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Ping ×1").clicked() {
                go = 1;
            }
            if self.mode == Mode::Realtime && ui.button("Ping ×4").clicked() {
                go = 4;
            }
            // Scorciatoia: destinazioni note nella rete.
            let targets: Vec<(String, Ipv4Addr)> = self
                .net
                .devices
                .values()
                .filter(|d| Some(d.id) != self.ping_src)
                .flat_map(|d| d.interfaces.iter().filter_map(move |i| i.ip.map(|c| (d.name.clone(), c.addr))))
                .collect();
            egui::ComboBox::from_id_salt("ping-pick").selected_text("scegli…").show_ui(ui, |ui| {
                for (name, ip) in targets {
                    if ui.selectable_label(false, format!("{name} — {ip}")).clicked() {
                        self.ping_dst = ip.to_string();
                    }
                }
            });
        });
        if go > 0 {
            self.do_ping(go);
        }
        if self.mode == Mode::Simulation {
            ui.add_space(4.0);
            self.ping_results(ui);
        }
    }

    fn do_ping(&mut self, count: usize) {
        let Some(src) = self.ping_src else {
            self.status = "Scegli il dispositivo sorgente del ping".into();
            return;
        };
        let Ok(dst) = self.ping_dst.trim().parse::<Ipv4Addr>() else {
            self.status = format!("IP di destinazione non valido: '{}'", self.ping_dst);
            return;
        };
        for _ in 0..count {
            if let Err(e) = self.net.ping(src, dst) {
                self.status = e;
                return;
            }
            if self.mode == Mode::Realtime {
                self.net.run_until_idle(10_000);
            }
        }
        if self.mode == Mode::Simulation {
            self.sim_progress = 0.0;
            self.status = "Pacchetto creato: premi ▶ Riproduci o Avanti".into();
        } else {
            let ok = self.net.pings.iter().rev().take(count).filter(|p| matches!(p.status, PingStatus::Success { .. })).count();
            self.status = format!("Ping verso {dst}: {ok}/{count} risposte");
        }
    }

    fn ping_results(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Risultati ping").strong());
            if ui.small_button("Pulisci").clicked() {
                self.net.pings.clear();
            }
        });
        egui::ScrollArea::vertical().id_salt("pings").stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            for p in &self.net.pings {
                let src = self.net.device_name(p.src);
                let (color, text) = match &p.status {
                    PingStatus::Pending => (Color32::GRAY, format!("{src} → {}: in corso…", p.dst)),
                    PingStatus::Success { ticks, ttl } => {
                        (LINK_UP, format!("{src} → {}: Reply bytes=32 hop-time={ticks} TTL={ttl}", p.dst))
                    }
                    PingStatus::Failed(r) => (LINK_DOWN, format!("{src} → {}: {r}", p.dst)),
                };
                ui.colored_label(color, RichText::new(text).monospace());
            }
        });
    }

    fn sim_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Simulazione").strong());
            let play = if self.sim_playing { "⏸ Pausa" } else { "▶ Riproduci" };
            if ui.button(play).clicked() {
                self.sim_playing = !self.sim_playing;
            }
            if ui.button("⏭ Avanti").clicked() {
                self.sim_playing = false;
                self.sim_progress = 0.0;
                self.net.step();
            }
            if ui.button("⟲ Reset").clicked() {
                self.sim_playing = false;
                self.sim_progress = 0.0;
                self.net.reset_runtime();
                self.selected_event = None;
            }
            ui.add(egui::Slider::new(&mut self.sim_speed, 0.25..=6.0).text("velocità"));
        });
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            let row_h = 18.0;
            let n = self.net.events.len();
            egui::ScrollArea::vertical().id_salt("events").stick_to_bottom(true).auto_shrink([false, false]).show_rows(
                ui,
                row_h,
                n,
                |ui, range| {
                    for i in range {
                        let e = &self.net.events[i];
                        let color = match e.kind {
                            EventKind::Dropped | EventKind::Failure => LINK_DOWN,
                            EventKind::Success => LINK_UP,
                            _ => ui.visuals().text_color(),
                        };
                        let proto = e.frame.as_ref().map(|f| f.protocol()).unwrap_or("");
                        let text = format!(
                            "{:>3} {:<9} {:<5} {}: {}",
                            e.tick,
                            e.kind.label(),
                            proto,
                            self.net.device_name(e.device),
                            e.info
                        );
                        let r = ui.selectable_label(
                            self.selected_event == Some(i),
                            RichText::new(text).monospace().size(11.5).color(color),
                        );
                        if r.clicked() {
                            self.selected_event = Some(i);
                        }
                    }
                },
            );
            let ui = &mut cols[1];
            egui::ScrollArea::vertical().id_salt("pdu").auto_shrink([false, false]).show(ui, |ui| {
                match self.selected_event.and_then(|i| self.net.events.get(i)).and_then(|e| e.frame.as_ref()) {
                    Some(f) => {
                        ui.label(RichText::new(f.summary()).strong());
                        for layer in f.layers() {
                            egui::CollapsingHeader::new(format!("Livello {} — {}", layer.layer, layer.title))
                                .default_open(true)
                                .show(ui, |ui| {
                                    for (k, v) in layer.fields {
                                        ui.label(RichText::new(format!("{k}: {v}")).monospace().size(11.5));
                                    }
                                });
                        }
                    }
                    None => {
                        ui.label("Clicca un evento con un pacchetto per ispezionarne i livelli.");
                    }
                }
            });
        });
    }

    fn sim_animate(&mut self, ctx: &egui::Context) {
        if self.mode != Mode::Simulation || !self.sim_playing {
            return;
        }
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        self.sim_progress += dt * self.sim_speed;
        if self.sim_progress >= 1.0 {
            self.sim_progress = 0.0;
            self.net.step();
            if self.net.is_idle() {
                self.sim_playing = false;
                self.status = "Simulazione completata".into();
            }
        }
        ctx.request_repaint();
    }
}

/// Accetta "255.255.255.0", "/24", "24" o vuoto (classe di default).
fn parse_mask(mask: &str, ip: Ipv4Addr) -> Result<u8, String> {
    let m = mask.trim().trim_start_matches('/');
    if m.is_empty() {
        return Ok(default_prefix_for(ip));
    }
    if let Ok(p) = m.parse::<u8>() {
        return if p <= 32 { Ok(p) } else { Err(format!("prefisso non valido: {p}")) };
    }
    let dotted: Ipv4Addr = m.parse().map_err(|_| format!("subnet mask non valida: {mask}"))?;
    mask_to_prefix(dotted)
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 == 0.0 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}
