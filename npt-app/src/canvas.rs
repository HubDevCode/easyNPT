//! Palette degli strumenti e area di lavoro.

use crate::app::{Mode, NptApp, Selection, Tool, ACCENT, LINK_DOWN, LINK_UP, SELECT};
use crate::icons::{self, ICON_SIZE};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};
use npt_core::device::short_iface_name;
use npt_core::sim::{CableKind, Note};
use npt_core::{DeviceId, DeviceKind, Endpoint, LinkId};

const GRID: Color32 = Color32::from_rgb(228, 232, 238);

impl NptApp {
    // ------------------------------------------------------------------
    // Palette a sinistra
    // ------------------------------------------------------------------

    pub fn left_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("tools").resizable(false).exact_size(138.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("Strumenti").strong());
                let tools = [
                    (Tool::Select, "🖱 Seleziona"),
                    (Tool::Cable, "🔌 Cavo"),
                    (Tool::Pdu, "✉ PDU semplice"),
                    (Tool::Note, "🗒 Nota"),
                    (Tool::Delete, "🗑 Elimina"),
                ];
                for (tool, label) in tools {
                    if ui.add_sized([126.0, 24.0], egui::Button::selectable(self.tool == tool, label)).clicked() {
                        self.set_tool(tool);
                    }
                }
                for (title, kinds) in [
                    ("Dispositivi di rete", &[DeviceKind::Router, DeviceKind::Switch, DeviceKind::Hub][..]),
                    ("Dispositivi finali", &[DeviceKind::Pc, DeviceKind::Laptop, DeviceKind::Server][..]),
                ] {
                    ui.separator();
                    ui.label(RichText::new(title).strong());
                    for &kind in kinds {
                        self.palette_item(ui, kind);
                    }
                }
            });
        });
    }

    fn palette_item(&mut self, ui: &mut egui::Ui, kind: DeviceKind) {
        let selected = self.tool == Tool::Place(kind);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(126.0, 54.0), Sense::click());
        let bg = if selected {
            Color32::from_rgb(255, 236, 200)
        } else if resp.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 4.0, bg);
        icons::draw_device(ui.painter(), rect.center() - Vec2::new(0.0, 8.0), kind, 0.58);
        let text_color = if selected { Color32::BLACK } else { ui.visuals().text_color() };
        ui.painter().text(
            rect.center_bottom() - Vec2::new(0.0, 3.0),
            Align2::CENTER_BOTTOM,
            format!("{} ({})", kind.label(), kind.model()),
            FontId::proportional(11.0),
            text_color,
        );
        if resp.clicked() {
            self.set_tool(if selected { Tool::Select } else { Tool::Place(kind) });
        }
    }

    // ------------------------------------------------------------------
    // Area di lavoro
    // ------------------------------------------------------------------

    fn to_screen(&self, origin: Pos2, p: [f32; 2]) -> Pos2 {
        origin + self.pan + Vec2::new(p[0], p[1]) * self.zoom
    }

    fn to_world(&self, origin: Pos2, p: Pos2) -> [f32; 2] {
        let w = (p - origin - self.pan) / self.zoom;
        [w.x, w.y]
    }

    pub fn canvas(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let bg = ui.interact(rect, ui.scope_id().with("canvas-bg"), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let origin = rect.min;
        let z = self.zoom;

        // Zoom con Ctrl+rotella, centrato sul puntatore.
        if bg.hovered() || ui.rect_contains_pointer(rect) {
            let (scroll, ctrl, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.command, i.pointer.hover_pos()));
            if ctrl && scroll != 0.0 {
                if let Some(p) = pointer {
                    let before = self.to_world(origin, p);
                    self.zoom = (self.zoom * (1.0 + scroll * 0.0015)).clamp(0.3, 3.0);
                    let after = self.to_screen(origin, before);
                    self.pan += p - after;
                }
            }
        }

        // Griglia.
        let step = 40.0 * z;
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

        // Note.
        let mut note_clicked = None;
        let mut note_drag: Option<(usize, Vec2)> = None;
        for (i, note) in self.net.notes.iter().enumerate() {
            let p = self.to_screen(origin, note.pos);
            let galley = painter.layout(note.text.clone(), FontId::proportional(13.0 * z), Color32::from_gray(30), 260.0 * z);
            let r = Rect::from_min_size(p, galley.size() + Vec2::splat(12.0 * z));
            let selected = self.selection == Some(Selection::Note(i));
            painter.rect_filled(r, 3.0, Color32::from_rgb(255, 248, 196));
            painter.rect_stroke(
                r,
                3.0,
                Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { SELECT } else { Color32::from_rgb(220, 200, 120) }),
                egui::StrokeKind::Middle,
            );
            painter.galley(p + Vec2::splat(6.0 * z), galley, Color32::BLACK);
            let resp = ui.interact(r, ui.scope_id().with(("note", i)), Sense::click_and_drag());
            if resp.clicked() {
                note_clicked = Some(i);
            }
            if resp.dragged() && self.tool == Tool::Select {
                note_drag = Some((i, resp.drag_delta() / z));
            }
        }
        if let Some((i, d)) = note_drag {
            self.net.notes[i].pos[0] += d.x;
            self.net.notes[i].pos[1] += d.y;
            self.dirty = true;
        }

        // Cavi.
        let mut link_segments: Vec<(LinkId, Pos2, Pos2)> = Vec::new();
        for link in self.net.links.values() {
            let (Some(da), Some(db)) = (self.net.device(link.a.device), self.net.device(link.b.device)) else { continue };
            let pa = self.to_screen(origin, da.pos);
            let pb = self.to_screen(origin, db.pos);
            link_segments.push((link.id, pa, pb));
            let selected = self.selection == Some(Selection::Link(link.id));
            let color = if selected { SELECT } else { Color32::from_rgb(60, 60, 70) };
            let stroke = Stroke::new(if selected { 3.5 } else { 2.0 } * z.sqrt(), color);
            if link.cable == CableKind::CopperCross {
                painter.extend(Shape::dashed_line(&[pa, pb], stroke, 8.0 * z, 5.0 * z));
            } else {
                painter.line_segment([pa, pb], stroke);
            }
            for (ep, from, to) in [(link.a, pa, pb), (link.b, pb, pa)] {
                let dev = self.net.device(ep.device).unwrap();
                let ifc = &dev.interfaces[ep.iface];
                let dir = (to - from).normalized();
                let half_w = (ICON_SIZE.x / 2.0 + 6.0) * z;
                let half_h = if dir.y > 0.0 { ICON_SIZE.y / 2.0 + 38.0 } else { ICON_SIZE.y / 2.0 + 6.0 } * z;
                let exit = (half_w / dir.x.abs().max(1e-3)).min(half_h / dir.y.abs().max(1e-3));
                let d_light = (exit + 6.0 * z).min((to - from).length() * 0.35);
                let light = from + dir * d_light;
                let (_, proto) = self.net.port_status(ep);
                let color = if proto == "up" { LINK_UP } else { LINK_DOWN };
                painter.circle_filled(light, 5.0 * z.sqrt(), color);
                let perp = Vec2::new(-dir.y, dir.x);
                let perp = if perp.y > 0.0 { -perp } else { perp };
                if z > 0.55 {
                    painter.text(
                        light + dir * 14.0 * z + perp * 11.0 * z,
                        Align2::CENTER_CENTER,
                        short_iface_name(&ifc.name),
                        FontId::proportional(10.0 * z.max(0.8)),
                        Color32::from_gray(90),
                    );
                }
            }
        }

        // Pacchetti in transito (simulazione).
        if self.mode == Mode::Simulation {
            for f in &self.net.in_flight {
                let (Some(a), Some(b)) = (self.net.device(f.from.device), self.net.device(f.to.device)) else { continue };
                let pa = self.to_screen(origin, a.pos);
                let pb = self.to_screen(origin, b.pos);
                let p = pa + (pb - pa) * self.sim_progress.clamp(0.0, 1.0);
                let color = match f.frame.protocol() {
                    "ARP" => Color32::from_rgb(240, 190, 30),
                    "DHCP" => Color32::from_rgb(60, 170, 220),
                    "DNS" => Color32::from_rgb(90, 190, 120),
                    _ => Color32::from_rgb(150, 80, 200),
                };
                let env = Rect::from_center_size(p, Vec2::new(22.0, 15.0) * z.max(0.7));
                painter.rect_filled(env, 2.0, color);
                painter.rect_stroke(env, 2.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Middle);
                painter.line_segment([env.left_top(), env.center()], Stroke::new(1.0, Color32::BLACK));
                painter.line_segment([env.right_top(), env.center()], Stroke::new(1.0, Color32::BLACK));
            }
        }

        // Dispositivi.
        let ids: Vec<DeviceId> = self.net.devices.keys().copied().collect();
        let mut clicked = None;
        let mut double = None;
        for id in ids {
            let dev = self.net.device(id).unwrap();
            let center = self.to_screen(origin, dev.pos);
            let hit = Rect::from_center_size(center, (ICON_SIZE + Vec2::splat(8.0)) * z);
            let resp = ui.interact(hit, ui.scope_id().with(("device", id)), Sense::click_and_drag());
            let highlighted = self.selection == Some(Selection::Device(id))
                || self.cable_from == Some(id)
                || self.pdu_from == Some(id);
            if highlighted || resp.hovered() {
                let c = if highlighted { SELECT } else { Color32::from_gray(180) };
                painter.rect_stroke(hit, 6.0, Stroke::new(2.0, c), egui::StrokeKind::Middle);
            }
            icons::draw_device(&painter, center, dev.kind, z);
            painter.text(
                hit.center_bottom() + Vec2::new(0.0, 2.0),
                Align2::CENTER_TOP,
                &dev.name,
                FontId::proportional(13.0 * z.max(0.7)),
                Color32::from_gray(20),
            );
            if let Some(ip) = dev.primary_ip().filter(|_| z > 0.55) {
                painter.text(
                    hit.center_bottom() + Vec2::new(0.0, 17.0 * z.max(0.7)),
                    Align2::CENTER_TOP,
                    ip.to_string(),
                    FontId::proportional(10.5 * z.max(0.8)),
                    Color32::from_gray(90),
                );
            }
            if resp.dragged() && matches!(self.tool, Tool::Select | Tool::Place(_)) {
                let d = resp.drag_delta() / z;
                let dev = self.net.device_mut(id).unwrap();
                dev.pos[0] += d.x;
                dev.pos[1] += d.y;
                self.dirty = true;
            }
            if resp.double_clicked() {
                double = Some(id);
            } else if resp.clicked() {
                clicked = Some(id);
            }
            if self.tool == Tool::Select {
                let tip = self.device_tooltip(id);
                resp.on_hover_text(tip);
            }
        }
        if let Some(id) = double {
            self.selection = Some(Selection::Device(id));
            self.open_window(id);
        } else if let Some(id) = clicked {
            self.on_device_click(id);
        }
        if let Some(i) = note_clicked {
            match self.tool {
                Tool::Delete => {
                    self.net.notes.remove(i);
                    self.dirty = true;
                }
                _ => self.selection = Some(Selection::Note(i)),
            }
        }

        // Anteprima di cavo o PDU in corso.
        if let (Some(from), Some(pointer)) = (self.cable_from.or(self.pdu_from), ui.ctx().pointer_hover_pos()) {
            if let Some(d) = self.net.device(from) {
                let p = self.to_screen(origin, d.pos);
                let c = if self.pdu_from.is_some() { ACCENT } else { SELECT };
                painter.extend(Shape::dashed_line(&[p, pointer], Stroke::new(2.0, c), 6.0, 4.0));
            }
        }

        // Interazioni con lo sfondo.
        if (bg.dragged() && self.tool == Tool::Select) || bg.dragged_by(egui::PointerButton::Middle) {
            self.pan += bg.drag_delta();
        }
        if bg.clicked() && note_clicked.is_none() {
            if let Some(pos) = bg.interact_pointer_pos() {
                let world = self.to_world(origin, pos);
                let link_hit =
                    link_segments.iter().find(|(_, a, b)| dist_to_segment(pos, *a, *b) < 6.0).map(|(id, ..)| *id);
                match self.tool {
                    Tool::Place(kind) => {
                        let id = self.net.add_device(kind, world);
                        self.dirty = true;
                        self.selection = Some(Selection::Device(id));
                        self.status = format!("Aggiunto {}. Doppio clic per configurarlo.", self.net.device_name(id));
                    }
                    Tool::Delete => {
                        if let Some(l) = link_hit {
                            self.delete_link(l);
                        }
                    }
                    Tool::Note => {
                        self.net.notes.push(Note { pos: world, text: "Nuova nota".into() });
                        self.selection = Some(Selection::Note(self.net.notes.len() - 1));
                        self.dirty = true;
                        self.set_tool(Tool::Select);
                        self.side_tab = crate::app::SideTab::Info;
                    }
                    Tool::Cable | Tool::Pdu => {
                        self.cable_from = None;
                        self.pdu_from = None;
                    }
                    Tool::Select => self.selection = link_hit.map(Selection::Link),
                }
            }
        }

        if self.net.devices.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Scegli un dispositivo a sinistra e clicca qui per aggiungerlo.\nPoi usa lo strumento 🔌 Cavo per collegarli.\nDoppio clic su un dispositivo per configurarlo.",
                FontId::proportional(16.0),
                Color32::from_gray(140),
            );
        }
    }

    fn device_tooltip(&self, id: DeviceId) -> String {
        let dev = self.net.device(id).unwrap();
        let mut s = format!("{} — {} ({})", dev.name, dev.kind.label(), dev.kind.model());
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
            let state = if ifc.enabled { "" } else { " [spenta]" };
            s.push_str(&format!("\n{} {ip}{state} ↔ {peer}", short_iface_name(&ifc.name)));
        }
        if let Some(gw) = dev.gateway {
            s.push_str(&format!("\nGateway: {gw}"));
        }
        s.push_str("\n\nDoppio clic per aprire");
        s
    }

    fn on_device_click(&mut self, id: DeviceId) {
        match self.tool {
            Tool::Select | Tool::Place(_) | Tool::Note => {
                self.selection = Some(Selection::Device(id));
            }
            Tool::Delete => self.delete_device(id),
            Tool::Pdu => match self.pdu_from {
                None => {
                    if !self.net.device(id).unwrap().kind.has_ip() {
                        self.status = "Un hub non può inviare pacchetti".into();
                    } else {
                        self.pdu_from = Some(id);
                        self.status = format!("PDU da {}: clicca la destinazione", self.net.device_name(id));
                    }
                }
                Some(a) if a == id => self.pdu_from = None,
                Some(a) => {
                    self.pdu_from = None;
                    self.send_pdu(a, id);
                }
            },
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
                        (Some(ia), Some(ib)) => self.port_dialog = Some(crate::app::PortDialog { a, b: id, ia, ib }),
                        _ => self.status = format!("{} non ha porte libere", self.net.device_name(id)),
                    }
                }
            },
        }
    }

    pub fn port_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.port_dialog else { return };
        let mut action: Option<bool> = None;
        egui::Window::new("Collega con un cavo")
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
                            .width(200.0)
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
                    if ui.button("Collega").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        action = Some(true);
                    }
                    if ui.button("Annulla").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
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
                    }
                    Err(e) => self.status = format!("Errore: {e}"),
                }
                self.port_dialog = None;
            }
            Some(false) => self.port_dialog = None,
            None => {}
        }
    }
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
