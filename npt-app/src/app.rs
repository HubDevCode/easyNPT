//! Stato dell'applicazione, barra superiore e disposizione dei pannelli.

use crate::devwin::DevWin;
use eframe::egui::{self, Color32, RichText, Vec2};
use npt_core::{DeviceId, DeviceKind, LinkId, Network};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;

pub const LINK_UP: Color32 = Color32::from_rgb(40, 170, 80);
pub const LINK_DOWN: Color32 = Color32::from_rgb(210, 60, 50);
pub const SELECT: Color32 = Color32::from_rgb(255, 160, 0);
pub const ACCENT: Color32 = Color32::from_rgb(55, 110, 190);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Place(DeviceKind),
    Cable,
    Delete,
    /// "Simple PDU": clic sulla sorgente, poi sulla destinazione.
    Pdu,
    Note,
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
    Note(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SideTab {
    Info,
    Simulation,
    Pdus,
}

pub struct PortDialog {
    pub a: DeviceId,
    pub b: DeviceId,
    pub ia: usize,
    pub ib: usize,
}

/// PDU creata con lo strumento "Simple PDU".
pub struct UserPdu {
    pub src: DeviceId,
    pub dst: DeviceId,
    pub dst_ip: Ipv4Addr,
    pub ping_index: usize,
}

pub struct NptApp {
    pub net: Network,
    pub tool: Tool,
    pub mode: Mode,
    pub selection: Option<Selection>,
    pub cable_from: Option<DeviceId>,
    pub pdu_from: Option<DeviceId>,
    pub port_dialog: Option<PortDialog>,
    pub pan: Vec2,
    pub zoom: f32,
    pub side_tab: SideTab,
    pub windows: BTreeMap<DeviceId, DevWin>,
    pub pdus: Vec<UserPdu>,
    pub sim_playing: bool,
    pub sim_progress: f32,
    pub sim_speed: f32,
    pub selected_event: Option<usize>,
    pub file: Option<PathBuf>,
    pub dirty: bool,
    pub status: String,
    last_title: String,
    show_about: bool,
    show_help: bool,
}

impl NptApp {
    pub fn new() -> Self {
        Self {
            net: Network::new(),
            tool: Tool::Select,
            mode: Mode::Realtime,
            selection: None,
            cable_from: None,
            pdu_from: None,
            port_dialog: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            side_tab: SideTab::Info,
            windows: BTreeMap::new(),
            pdus: Vec::new(),
            sim_playing: false,
            sim_progress: 0.0,
            sim_speed: 1.5,
            selected_event: None,
            file: None,
            dirty: false,
            status: "Pronto. Scegli un dispositivo a sinistra e clicca sull'area di lavoro.".into(),
            last_title: String::new(),
            show_about: false,
            show_help: false,
        }
    }

    pub fn open_path(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| Network::from_json(&s)) {
            Ok(net) => {
                *self = Self::new();
                self.net = net;
                self.status = format!("Aperto {}", path.display());
                self.file = Some(path);
            }
            Err(e) => self.status = format!("Errore apertura: {e}"),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.handle_keys(&ctx);
        self.top_bar(ui);
        self.status_bar(ui);
        self.left_panel(ui);
        self.side_panel(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(244, 246, 249)))
            .show(ui, |ui| self.canvas(ui));
        self.device_windows(&ctx);
        self.port_dialog_window(&ctx);
        self.about_window(&ctx);
        self.help_window(&ctx);
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
    // Azioni comuni
    // ------------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (save, open, new) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::N),
            )
        });
        if save {
            self.save(false);
        }
        if open {
            self.open_dialog();
        }
        if new {
            self.new_file();
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (del, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Delete), i.key_pressed(egui::Key::Escape)));
        if del {
            match self.selection.take() {
                Some(Selection::Device(d)) => self.delete_device(d),
                Some(Selection::Link(l)) => self.delete_link(l),
                Some(Selection::Note(n)) => {
                    if n < self.net.notes.len() {
                        self.net.notes.remove(n);
                        self.dirty = true;
                    }
                }
                None => {}
            }
        }
        if esc {
            self.set_tool(Tool::Select);
        }
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.cable_from = None;
        self.pdu_from = None;
        self.status = match tool {
            Tool::Select => "Seleziona e trascina i dispositivi. Doppio clic per aprirne la configurazione.".into(),
            Tool::Place(k) => format!("Clicca sull'area di lavoro per aggiungere un {}", k.label()),
            Tool::Cable => "Cavo: clicca il primo dispositivo, poi il secondo.".into(),
            Tool::Delete => "Elimina: clicca un dispositivo, un cavo o una nota.".into(),
            Tool::Pdu => "PDU semplice (ping): clicca la sorgente, poi la destinazione.".into(),
            Tool::Note => "Nota: clicca sull'area di lavoro per aggiungere una nota.".into(),
        };
    }

    pub fn delete_device(&mut self, d: DeviceId) {
        let name = self.net.device_name(d);
        self.net.remove_device(d);
        self.windows.remove(&d);
        self.pdus.retain(|p| p.src != d && p.dst != d);
        self.dirty = true;
        self.status = format!("Eliminato {name}");
    }

    pub fn delete_link(&mut self, l: LinkId) {
        self.net.disconnect(l);
        self.dirty = true;
        self.status = "Cavo rimosso".into();
    }

    /// Apre (o porta in primo piano) la finestra di un dispositivo.
    pub fn open_window(&mut self, id: DeviceId) {
        if !self.windows.contains_key(&id) {
            let w = DevWin::new(&self.net, id);
            self.windows.insert(id, w);
        }
    }

    /// Apre la finestra di un dispositivo per nome; `Nome@scheda` sceglie la scheda.
    pub fn open_window_by_name(&mut self, spec: &str) {
        let (name, tab) = spec.split_once('@').unwrap_or((spec, ""));
        if let Some(id) = self.net.device_by_name(name) {
            self.selection = Some(Selection::Device(id));
            self.open_window(id);
            if let Some(w) = self.windows.get_mut(&id) {
                w.select_tab(tab);
            }
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        match mode {
            Mode::Simulation => {
                self.net.events.clear();
                self.selected_event = None;
                self.side_tab = SideTab::Simulation;
                self.status = "Simulazione: crea una PDU (✉) o un ping, poi usa ▶ o ⏭.".into();
            }
            Mode::Realtime => {
                self.sim_playing = false;
                self.net.run_until_idle(npt_core::sim::REALTIME_MAX_TICKS);
                if self.side_tab == SideTab::Simulation {
                    self.side_tab = SideTab::Info;
                }
                self.status = "Tempo reale".into();
            }
        }
    }

    /// Esegue (o accoda, in simulazione) un ping da `src` all'IP di `dst`.
    pub fn send_pdu(&mut self, src: DeviceId, dst: DeviceId) {
        let Some(dst_ip) = self.net.device(dst).and_then(|d| d.primary_ip()).map(|c| c.addr) else {
            self.status = format!("{} non ha un indirizzo IP", self.net.device_name(dst));
            return;
        };
        match self.net.ping_ttl(src, dst_ip, None) {
            Ok(idx) => {
                self.pdus.push(UserPdu { src, dst, dst_ip, ping_index: idx });
                if self.mode == Mode::Realtime {
                    self.net.run_until_idle(npt_core::sim::REALTIME_MAX_TICKS);
                } else {
                    self.sim_progress = 0.0;
                    self.side_tab = SideTab::Simulation;
                }
                self.status = format!("PDU: {} → {} ({dst_ip})", self.net.device_name(src), self.net.device_name(dst));
            }
            Err(e) => self.status = e,
        }
    }

    // ------------------------------------------------------------------
    // Barra superiore e di stato
    // ------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.label(RichText::new("NPT").strong().size(18.0).color(ACCENT));
                ui.label(RichText::new("easy").italics().size(13.0));
                ui.separator();
                ui.menu_button("File", |ui| {
                    if ui.button("Nuovo            Ctrl+N").clicked() {
                        self.new_file();
                    }
                    if ui.button("Apri…            Ctrl+O").clicked() {
                        self.open_dialog();
                    }
                    if ui.button("Salva            Ctrl+S").clicked() {
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
                ui.menu_button("Modifica", |ui| {
                    if ui.button("Elimina selezione   Canc").clicked() {
                        match self.selection.take() {
                            Some(Selection::Device(d)) => self.delete_device(d),
                            Some(Selection::Link(l)) => self.delete_link(l),
                            Some(Selection::Note(n)) if n < self.net.notes.len() => {
                                self.net.notes.remove(n);
                            }
                            _ => {}
                        }
                    }
                    if ui.button("Azzera tabelle ARP/MAC di tutti").clicked() {
                        self.net.reset_runtime();
                        self.pdus.clear();
                        self.status = "Stato dinamico della rete azzerato".into();
                    }
                });
                ui.menu_button("Visualizza", |ui| {
                    if ui.button("Zoom +").clicked() {
                        self.zoom = (self.zoom * 1.2).min(3.0);
                    }
                    if ui.button("Zoom −").clicked() {
                        self.zoom = (self.zoom / 1.2).max(0.3);
                    }
                    if ui.button("Zoom 100% / centra").clicked() {
                        self.zoom = 1.0;
                        self.pan = Vec2::ZERO;
                    }
                });
                ui.menu_button("Aiuto", |ui| {
                    if ui.button("Guida rapida").clicked() {
                        self.show_help = true;
                    }
                    if ui.button("Informazioni su NPT easy").clicked() {
                        self.show_about = true;
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.selectable_label(self.mode == Mode::Simulation, "⏱ Simulazione").clicked() {
                        self.set_mode(Mode::Simulation);
                    }
                    if ui.selectable_label(self.mode == Mode::Realtime, "⚡ Tempo reale").clicked() {
                        self.set_mode(Mode::Realtime);
                    }
                });
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").exact_size(24.0).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!(
                        "{} dispositivi · {} cavi · zoom {:.0}% · tick {}",
                        self.net.devices.len(),
                        self.net.links.len(),
                        self.zoom * 100.0,
                        self.net.tick
                    ));
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

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        egui::Window::new("Guida rapida").open(&mut open).default_width(460.0).show(ctx, |ui| {
            ui.label(RichText::new("Costruire la rete").strong());
            ui.label("• Scegli un dispositivo nella colonna di sinistra e clicca sull'area di lavoro.");
            ui.label("• Strumento 🔌 Cavo: clicca due dispositivi e scegli le porte.");
            ui.label("• Doppio clic su un dispositivo: finestra con Config, CLI (router/switch), Desktop (PC) e Servizi (server).");
            ui.add_space(6.0);
            ui.label(RichText::new("Verificare").strong());
            ui.label("• Strumento ✉ PDU: clicca sorgente e destinazione per inviare un ping.");
            ui.label("• Desktop → Prompt dei comandi: ping, tracert, ipconfig, arp -a, nslookup.");
            ui.label("• Modalità ⏱ Simulazione: i pacchetti si muovono passo passo; clicca un evento per vederne i livelli OSI.");
            ui.add_space(6.0);
            ui.label(RichText::new("Scorciatoie").strong());
            ui.label("Canc elimina · Esc annulla lo strumento · Ctrl+rotella zoom · trascina lo sfondo per spostare la vista");
        });
        self.show_help = open;
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
