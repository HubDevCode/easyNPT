//! Pannello laterale destro: informazioni, simulazione, elenco PDU.

use crate::app::{Mode, NptApp, Selection, SideTab, LINK_DOWN, LINK_UP};
use crate::icons;
use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use npt_core::device::{short_iface_name, RouteKind};
use npt_core::sim::EventKind;
use npt_core::{DeviceId, DeviceKind, Endpoint, LinkId, PingStatus};

impl NptApp {
    pub fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("side").resizable(true).default_size(380.0).show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.side_tab, SideTab::Info, "ℹ Dettagli");
                ui.selectable_value(&mut self.side_tab, SideTab::Simulation, "⏱ Simulazione");
                let n = self.pdus.len();
                ui.selectable_value(&mut self.side_tab, SideTab::Pdus, format!("✉ PDU ({n})"));
            });
            ui.separator();
            match self.side_tab {
                SideTab::Info => {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.info_tab(ui));
                }
                SideTab::Simulation => self.sim_tab(ui),
                SideTab::Pdus => self.pdu_tab(ui),
            }
        });
    }

    // ------------------------------------------------------------------
    // Dettagli della selezione
    // ------------------------------------------------------------------

    fn info_tab(&mut self, ui: &mut egui::Ui) {
        match self.selection {
            Some(Selection::Device(id)) if self.net.device(id).is_some() => self.device_info(ui, id),
            Some(Selection::Link(l)) if self.net.links.contains_key(&l) => self.link_info(ui, l),
            Some(Selection::Note(n)) if n < self.net.notes.len() => {
                ui.label(RichText::new("Nota").strong().size(16.0));
                if ui.add(egui::TextEdit::multiline(&mut self.net.notes[n].text).desired_rows(6).desired_width(f32::INFINITY)).changed() {
                    self.dirty = true;
                }
                if ui.button("🗑 Elimina nota").clicked() {
                    self.net.notes.remove(n);
                    self.selection = None;
                    self.dirty = true;
                }
            }
            _ => {
                ui.label(RichText::new("Nessuna selezione").strong().size(16.0));
                ui.label("Clicca un dispositivo o un cavo per vederne i dettagli.");
                ui.label("Doppio clic su un dispositivo per configurarlo (Config, CLI, Desktop).");
                ui.add_space(10.0);
                ui.label(RichText::new("Rete").strong());
                let mut counts: Vec<(DeviceKind, usize)> = DeviceKind::ALL
                    .iter()
                    .map(|k| (*k, self.net.devices.values().filter(|d| d.kind == *k).count()))
                    .collect();
                counts.retain(|c| c.1 > 0);
                for (k, n) in counts {
                    ui.label(format!("{}: {n}", k.label()));
                }
                ui.label(format!("Cavi: {}", self.net.links.len()));
            }
        }
    }

    fn link_info(&mut self, ui: &mut egui::Ui, l: LinkId) {
        let link = self.net.links[&l].clone();
        ui.label(RichText::new("Cavo").strong().size(16.0));
        ui.label(format!("Tipo: {}", link.cable.label()));
        for ep in [link.a, link.b] {
            let (st, proto) = self.net.port_status(ep);
            ui.label(format!("{}  —  {st}/{proto}", self.net.endpoint_label(ep)));
        }
        let up = self.net.link_is_up(&link);
        ui.colored_label(if up { LINK_UP } else { LINK_DOWN }, if up { "Collegamento attivo" } else { "Collegamento inattivo: una porta è spenta" });
        ui.add_space(8.0);
        if ui.button("🗑 Rimuovi cavo").clicked() {
            self.delete_link(l);
            self.selection = None;
        }
    }

    fn device_info(&mut self, ui: &mut egui::Ui, id: DeviceId) {
        let dev = self.net.device(id).unwrap().clone();
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(46.0, 36.0), Sense::hover());
            icons::draw_device(ui.painter(), r.center(), dev.kind, 0.65);
            ui.vertical(|ui| {
                ui.label(RichText::new(&dev.name).strong().size(16.0));
                ui.label(format!("{} — {}", dev.kind.label(), dev.kind.model()));
            });
        });
        if ui.button("⚙ Apri configurazione (doppio clic)").clicked() {
            self.open_window(id);
        }
        ui.add_space(6.0);

        ui.label(RichText::new("Interfacce").strong());
        egui::Grid::new(("ifaces-info", id)).striped(true).num_columns(4).show(ui, |ui| {
            ui.label(RichText::new("Porta").small());
            ui.label(RichText::new("Stato").small());
            ui.label(RichText::new("IP / VLAN").small());
            ui.label(RichText::new("Collegata a").small());
            ui.end_row();
            for (i, ifc) in dev.interfaces.iter().enumerate() {
                let ep = Endpoint { device: id, iface: i };
                if dev.kind.is_l2() && ifc.is_physical() && ifc.link.is_none() && ifc.enabled && ifc.switchport.access_vlan == 1 {
                    continue; // porte di switch inutilizzate: non intasiamo l'elenco
                }
                let (st, proto) = self.net.port_status(ep);
                let color = if proto == "up" { LINK_UP } else { LINK_DOWN };
                ui.label(short_iface_name(&ifc.name));
                ui.colored_label(color, if st == "administratively down" { "spenta".to_string() } else { format!("{st}/{proto}") });
                let ipv = if dev.kind.is_l2() && ifc.is_physical() {
                    match ifc.switchport.mode {
                        npt_core::device::PortMode::Access => format!("access {}", ifc.switchport.access_vlan),
                        npt_core::device::PortMode::Trunk => "trunk".into(),
                    }
                } else {
                    ifc.ip.map(|c| c.to_string()).unwrap_or_else(|| "—".into())
                };
                ui.label(ipv);
                let peer = self
                    .net
                    .peer_of(ep)
                    .map(|(d, p)| self.net.endpoint_label(Endpoint { device: d, iface: p }))
                    .unwrap_or_else(|| "—".into());
                ui.label(peer);
                ui.end_row();
            }
        });
        if let Some(gw) = dev.gateway {
            ui.label(format!("Gateway: {gw}"));
        }
        if let Some(dns) = dev.dns_server {
            ui.label(format!("Server DNS: {dns}"));
        }

        ui.add_space(6.0);
        if dev.kind.has_ip() {
            egui::CollapsingHeader::new("Tabella di routing").id_salt(("rt", id)).default_open(dev.kind.is_router()).show(ui, |ui| {
                let table = dev.routing_table();
                if table.is_empty() {
                    ui.label("vuota");
                }
                for r in table {
                    let code = match r.kind {
                        RouteKind::Connected => "C ",
                        RouteKind::Local => "L ",
                        RouteKind::Static => "S ",
                        RouteKind::Default => "S*",
                    };
                    let via = r.next_hop.map(|n| format!("via {n}")).unwrap_or_else(|| "connessa".into());
                    let ifn = short_iface_name(&dev.interfaces[r.iface].name);
                    ui.label(RichText::new(format!("{code} {} {via}, {ifn}", r.network)).monospace());
                }
            });
            egui::CollapsingHeader::new("Tabella ARP").id_salt(("arp", id)).show(ui, |ui| {
                if dev.state.arp_table.is_empty() {
                    ui.label("vuota");
                }
                for (ip, mac) in &dev.state.arp_table {
                    ui.label(RichText::new(format!("{:<16}{mac}", ip.to_string())).monospace());
                }
            });
        }
        if dev.kind == DeviceKind::Switch {
            egui::CollapsingHeader::new("Tabella MAC").id_salt(("mac", id)).default_open(true).show(ui, |ui| {
                if dev.state.mac_table.is_empty() {
                    ui.label("vuota (si riempie quando passano frame)");
                }
                for ((vlan, mac), port) in &dev.state.mac_table {
                    ui.label(RichText::new(format!("VLAN {vlan:<5}{mac}  {}", short_iface_name(&dev.interfaces[*port].name))).monospace());
                }
            });
            egui::CollapsingHeader::new("VLAN").id_salt(("vlans", id)).show(ui, |ui| {
                for (v, name) in &dev.vlans {
                    ui.label(RichText::new(format!("{v:<6}{name}")).monospace());
                }
            });
        }
        if !dev.dhcp_leases.is_empty() {
            egui::CollapsingHeader::new("Indirizzi DHCP assegnati").id_salt(("leases", id)).show(ui, |ui| {
                for l in &dev.dhcp_leases {
                    ui.label(RichText::new(format!("{:<16}{}", l.ip.to_string(), l.mac)).monospace());
                }
            });
        }
    }

    // ------------------------------------------------------------------
    // Simulazione
    // ------------------------------------------------------------------

    fn sim_tab(&mut self, ui: &mut egui::Ui) {
        if self.mode != Mode::Simulation {
            ui.label("Sei in modalità tempo reale: i pacchetti arrivano istantaneamente.");
            if ui.button("⏱ Passa alla modalità simulazione").clicked() {
                self.set_mode(Mode::Simulation);
            }
            return;
        }
        ui.horizontal_wrapped(|ui| {
            let play = if self.sim_playing { "⏸ Pausa" } else { "▶ Riproduci" };
            if ui.button(play).clicked() {
                self.sim_playing = !self.sim_playing;
            }
            if ui.button("⏭ Avanti").on_hover_text("Avanza di un passo").clicked() {
                self.sim_playing = false;
                self.sim_progress = 0.0;
                self.net.step();
            }
            if ui.button("⟲ Reset").on_hover_text("Cancella eventi, pacchetti e tabelle dinamiche").clicked() {
                self.sim_playing = false;
                self.sim_progress = 0.0;
                self.net.reset_runtime();
                self.pdus.clear();
                self.selected_event = None;
            }
        });
        ui.add(egui::Slider::new(&mut self.sim_speed, 0.25..=6.0).text("velocità"));
        ui.horizontal(|ui| {
            for (name, c) in [
                ("ICMP", Color32::from_rgb(150, 80, 200)),
                ("ARP", Color32::from_rgb(240, 190, 30)),
                ("DHCP", Color32::from_rgb(60, 170, 220)),
                ("DNS", Color32::from_rgb(90, 190, 120)),
            ] {
                let (r, _) = ui.allocate_exact_size(Vec2::new(12.0, 12.0), Sense::hover());
                ui.painter().rect_filled(r, 2.0, c);
                ui.label(RichText::new(name).small());
            }
        });
        ui.separator();
        ui.label(RichText::new("Elenco eventi").strong());
        let avail = ui.available_height();
        let row_h = 18.0;
        let n = self.net.events.len();
        egui::ScrollArea::vertical()
            .id_salt("events")
            .max_height(avail * 0.55)
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show_rows(ui, row_h, n, |ui, range| {
                for i in range {
                    let e = &self.net.events[i];
                    let color = match e.kind {
                        EventKind::Dropped | EventKind::Failure => LINK_DOWN,
                        EventKind::Success => LINK_UP,
                        _ => ui.visuals().text_color(),
                    };
                    let proto = e.frame.as_ref().map(|f| f.protocol()).unwrap_or("");
                    let text = format!("{:>3} {:<5} {}: {}", e.tick, proto, self.net.device_name(e.device), e.info);
                    let r = ui.selectable_label(self.selected_event == Some(i), RichText::new(text).monospace().size(11.5).color(color));
                    if r.clicked() {
                        self.selected_event = Some(i);
                    }
                }
            });
        ui.separator();
        ui.label(RichText::new("Dettagli PDU (livelli OSI)").strong());
        egui::ScrollArea::vertical().id_salt("pdu").auto_shrink([false, false]).show(ui, |ui| {
            match self.selected_event.and_then(|i| self.net.events.get(i)) {
                Some(e) => {
                    ui.label(format!("{} — {}: {}", e.kind.label(), self.net.device_name(e.device), e.info));
                    match &e.frame {
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
                            ui.label("(evento senza pacchetto)");
                        }
                    }
                }
                None => {
                    ui.label("Clicca un evento per vederne il pacchetto livello per livello.");
                }
            }
        });
    }

    // ------------------------------------------------------------------
    // PDU utente
    // ------------------------------------------------------------------

    fn pdu_tab(&mut self, ui: &mut egui::Ui) {
        ui.label("PDU create con lo strumento ✉ (ping da sorgente a destinazione).");
        ui.horizontal(|ui| {
            if ui.button("✉ Nuova PDU").clicked() {
                self.set_tool(crate::app::Tool::Pdu);
            }
            if ui.button("🔁 Rinvia tutte").clicked() {
                let list: Vec<(DeviceId, DeviceId)> = self.pdus.iter().map(|p| (p.src, p.dst)).collect();
                self.pdus.clear();
                for (s, d) in list {
                    self.send_pdu(s, d);
                }
            }
            if ui.button("🗑 Elimina tutte").clicked() {
                self.pdus.clear();
            }
        });
        ui.separator();
        let mut remove = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("pdus").striped(true).num_columns(5).show(ui, |ui| {
                for h in ["Esito", "Sorgente", "Destinazione", "Tipo", ""] {
                    ui.label(RichText::new(h).small());
                }
                ui.end_row();
                for (i, p) in self.pdus.iter().enumerate() {
                    let rec = self.net.pings.get(p.ping_index);
                    let (txt, color, tip) = match rec.map(|r| &r.status) {
                        Some(PingStatus::Success { .. }) => ("Riuscito", LINK_UP, String::new()),
                        Some(PingStatus::Failed(r)) => ("Fallito", LINK_DOWN, r.clone()),
                        _ => ("In corso", Color32::GRAY, String::new()),
                    };
                    let l = ui.colored_label(color, txt);
                    if !tip.is_empty() {
                        l.on_hover_text(tip);
                    }
                    ui.label(self.net.device_name(p.src));
                    ui.label(format!("{} ({})", self.net.device_name(p.dst), p.dst_ip));
                    ui.label("ICMP");
                    if ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(i) = remove {
            self.pdus.remove(i);
        }
    }
}
