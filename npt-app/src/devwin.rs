//! Finestre dei dispositivi (doppio clic): Config, CLI, Desktop, Servizi.

use crate::app::{Mode, NptApp, LINK_DOWN, LINK_UP};
use eframe::egui::{self, Color32, FontId, RichText, Vec2};
use npt_core::addr::{default_prefix_for, mask_to_prefix};
use npt_core::cli::{CliMode, CliSession};
use npt_core::device::{format_vlan_list, short_iface_name, DhcpPool, IfaceKind, PortMode};
use npt_core::hostcmd::{run_host_command, HOST_PROMPT};
use npt_core::sim::{validate_host_ip, REALTIME_MAX_TICKS};
use npt_core::{DeviceId, DeviceKind, Endpoint, Ipv4Cidr, Network};
use std::net::Ipv4Addr;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WinTab {
    Config,
    Cli,
    Desktop,
    Services,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Global,
    Routing,
    Vlans,
    Iface(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DesktopApp {
    Home,
    IpConfig,
    Prompt,
}

#[derive(Default)]
struct Console {
    log: String,
    input: String,
    history: Vec<String>,
    hist_pos: Option<usize>,
    focus: bool,
}

pub struct DevWin {
    tab: WinTab,
    section: Section,
    desktop: DesktopApp,
    cli: CliSession,
    cli_con: Console,
    host_con: Console,
    message: Option<(bool, String)>,
    ios_log: Vec<String>,
    // Campi dei moduli
    loaded: Option<Section>,
    hostname: String,
    gateway: String,
    dns: String,
    if_ip: String,
    if_mask: String,
    if_desc: String,
    if_helper: String,
    if_vlan: String,
    if_native: String,
    if_allowed: String,
    route_net: String,
    route_mask: String,
    route_nh: String,
    vlan_id: String,
    vlan_name: String,
    ipc_dhcp: bool,
    // Servizi del server
    pool_sel: Option<usize>,
    pool_name: String,
    pool_gw: String,
    pool_dns: String,
    pool_start: String,
    pool_mask: String,
    pool_max: String,
    dns_name: String,
    dns_addr: String,
}

impl DevWin {
    pub fn new(net: &Network, id: DeviceId) -> Self {
        let kind = net.devices[&id].kind;
        let cli = CliSession::new();
        let mut cli_con = Console::default();
        if kind.has_cli() {
            cli_con.log = cli.greeting(net, id) + "\n";
            cli_con.focus = true;
        }
        let host_con = Console {
            log: "NPT easy — prompt dei comandi. Digita help per l'elenco dei comandi.\n\n".into(),
            ..Default::default()
        };
        let mut w = Self {
            tab: if kind.has_cli() { WinTab::Cli } else if kind.is_host() { WinTab::Desktop } else { WinTab::Config },
            section: Section::Global,
            desktop: DesktopApp::Home,
            cli,
            cli_con,
            host_con,
            message: None,
            ios_log: Vec::new(),
            loaded: None,
            hostname: String::new(),
            gateway: String::new(),
            dns: String::new(),
            if_ip: String::new(),
            if_mask: String::new(),
            if_desc: String::new(),
            if_helper: String::new(),
            if_vlan: String::new(),
            if_native: String::new(),
            if_allowed: String::new(),
            route_net: String::new(),
            route_mask: String::new(),
            route_nh: String::new(),
            vlan_id: String::new(),
            vlan_name: String::new(),
            ipc_dhcp: false,
            pool_sel: None,
            pool_name: "serverPool".into(),
            pool_gw: String::new(),
            pool_dns: String::new(),
            pool_start: String::new(),
            pool_mask: "255.255.255.0".into(),
            pool_max: "100".into(),
            dns_name: String::new(),
            dns_addr: String::new(),
        };
        w.load(net, id);
        w
    }

    /// Ricarica i campi dei moduli dalla configurazione corrente.
    fn load(&mut self, net: &Network, id: DeviceId) {
        let d = &net.devices[&id];
        self.hostname = d.name.clone();
        self.gateway = d.gateway.map(|g| g.to_string()).unwrap_or_default();
        self.dns = d.dns_server.map(|g| g.to_string()).unwrap_or_default();
        let i = match self.section {
            Section::Iface(i) if i < d.interfaces.len() => i,
            _ if d.kind.is_host() => 0,
            _ => usize::MAX,
        };
        if let Some(ifc) = d.interfaces.get(i) {
            self.if_ip = ifc.ip.map(|c| c.addr.to_string()).unwrap_or_default();
            self.if_mask = ifc.ip.map(|c| c.mask().to_string()).unwrap_or_default();
            self.if_desc = ifc.description.clone();
            self.if_helper = ifc.helper.map(|h| h.to_string()).unwrap_or_default();
            self.if_vlan = ifc.switchport.access_vlan.to_string();
            self.if_native = ifc.switchport.native_vlan.to_string();
            self.if_allowed = ifc.switchport.allowed.as_ref().map(|a| format_vlan_list(a)).unwrap_or_else(|| "all".into());
            self.ipc_dhcp = ifc.dhcp_client;
        }
        self.loaded = Some(self.section);
    }

    /// Esegue comandi IOS di configurazione e li registra nel log "comandi equivalenti".
    fn run_ios(&mut self, net: &mut Network, id: DeviceId, cmds: &[String]) -> bool {
        let mut s = CliSession { mode: CliMode::Config, history: Vec::new() };
        let mut errors = Vec::new();
        for c in cmds {
            let out = s.execute(net, id, c);
            self.ios_log.push(c.clone());
            if out.starts_with('%') && !out.contains("Creating vlan") {
                errors.push(format!("{c}: {out}"));
            }
        }
        if errors.is_empty() {
            self.message = Some((true, "Configurazione applicata".into()));
            true
        } else {
            self.message = Some((false, errors.join("\n")));
            false
        }
    }
}

impl DevWin {
    /// Seleziona una scheda per nome (usato da NPT_APRI=Nome@scheda).
    pub fn select_tab(&mut self, tab: &str) {
        match tab {
            "config" => self.tab = WinTab::Config,
            "cli" => self.tab = WinTab::Cli,
            "desktop" => self.tab = WinTab::Desktop,
            "ip" => {
                self.tab = WinTab::Desktop;
                self.desktop = DesktopApp::IpConfig;
            }
            "prompt" => {
                self.tab = WinTab::Desktop;
                self.desktop = DesktopApp::Prompt;
                self.host_con.focus = true;
            }
            "servizi" => self.tab = WinTab::Services,
            _ => {}
        }
    }
}

impl NptApp {
    pub fn device_windows(&mut self, ctx: &egui::Context) {
        let ids: Vec<DeviceId> = self.windows.keys().copied().collect();
        for id in ids {
            if self.net.device(id).is_none() {
                self.windows.remove(&id);
                continue;
            }
            let mut w = self.windows.remove(&id).unwrap();
            let mut open = true;
            let d = self.net.device(id).unwrap();
            let title = format!("{} — {}", d.name, d.kind.label());
            egui::Window::new(title)
                .id(egui::Id::unique(("devwin", id)))
                .open(&mut open)
                .default_size([640.0, 470.0])
                .min_width(480.0)
                .resizable(true)
                .show(ctx, |ui| self.device_window_ui(ui, id, &mut w));
            if open {
                self.windows.insert(id, w);
            }
        }
    }

    fn device_window_ui(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        let kind = self.net.devices[&id].kind;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut w.tab, WinTab::Config, "⚙ Config");
            if kind.has_cli() {
                ui.selectable_value(&mut w.tab, WinTab::Cli, "⌨ CLI");
            }
            if kind.is_host() {
                ui.selectable_value(&mut w.tab, WinTab::Desktop, "🖥 Desktop");
            }
            if kind == DeviceKind::Server {
                ui.selectable_value(&mut w.tab, WinTab::Services, "🛠 Servizi");
            }
        });
        ui.separator();
        match w.tab {
            WinTab::Config => self.config_tab(ui, id, w),
            WinTab::Cli => self.cli_tab(ui, id, w),
            WinTab::Desktop => self.desktop_tab(ui, id, w),
            WinTab::Services => self.services_tab(ui, id, w),
        }
    }

    // ------------------------------------------------------------------
    // Config
    // ------------------------------------------------------------------

    fn config_tab(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        let d = self.net.devices[&id].clone();
        if w.loaded != Some(w.section) {
            w.message = None;
            w.load(&self.net, id);
        }
        let ios_height = if d.kind.has_cli() { 90.0 } else { 0.0 };
        let body_h = (ui.available_height() - ios_height).clamp(180.0, 520.0);
        ui.horizontal_top(|ui| {
            // Elenco delle sezioni (colonna verticale).
            ui.vertical(|ui| {
                ui.set_width(150.0);
                egui::ScrollArea::vertical().id_salt(("sections", id)).max_height(body_h).show(ui, |ui| {
                    ui.set_width(140.0);
                    ui.label(RichText::new("GLOBALE").small().strong());
                    ui.selectable_value(&mut w.section, Section::Global, "Impostazioni");
                    if d.kind.is_router() {
                        ui.selectable_value(&mut w.section, Section::Routing, "Routing statico");
                    }
                    if d.kind == DeviceKind::Switch {
                        ui.selectable_value(&mut w.section, Section::Vlans, "Database VLAN");
                    }
                    ui.label(RichText::new("INTERFACCE").small().strong());
                    for (i, ifc) in d.interfaces.iter().enumerate() {
                        let (_, proto) = self.net.port_status(Endpoint { device: id, iface: i });
                        ui.horizontal(|ui| {
                            ui.colored_label(if proto == "up" { LINK_UP } else { LINK_DOWN }, "●");
                            ui.selectable_value(&mut w.section, Section::Iface(i), short_iface_name(&ifc.name));
                        });
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().id_salt(("cfgbody", id)).max_height(body_h).show(ui, |ui| {
                    match w.section {
                        Section::Global => self.cfg_global(ui, id, w),
                        Section::Routing => self.cfg_routing(ui, id, w),
                        Section::Vlans => self.cfg_vlans(ui, id, w),
                        Section::Iface(i) if i < d.interfaces.len() => self.cfg_iface(ui, id, i, w),
                        _ => w.section = Section::Global,
                    }
                    if let Some((ok, m)) = &w.message {
                        ui.add_space(6.0);
                        ui.colored_label(if *ok { LINK_UP } else { LINK_DOWN }, m);
                    }
                });
            });
        });
        if d.kind.has_cli() {
            ui.separator();
            ui.label(RichText::new("Comandi IOS equivalenti").small().strong());
            egui::ScrollArea::vertical().id_salt(("ioslog", id)).max_height(ios_height - 24.0).stick_to_bottom(true).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for c in &w.ios_log {
                    ui.label(RichText::new(c).monospace().size(11.0));
                }
            });
        }
    }

    fn cfg_global(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        let kind = self.net.devices[&id].kind;
        ui.label(RichText::new("Impostazioni globali").strong().size(15.0));
        egui::Grid::new(("glob", id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(if kind.has_cli() { "Hostname:" } else { "Nome visualizzato:" });
            ui.text_edit_singleline(&mut w.hostname);
            ui.end_row();
            if kind.is_host() || kind == DeviceKind::Switch {
                ui.label("Default gateway:");
                ui.add(egui::TextEdit::singleline(&mut w.gateway).hint_text("es. 192.168.1.1"));
                ui.end_row();
            }
            if kind.is_host() {
                ui.label("Server DNS:");
                ui.add(egui::TextEdit::singleline(&mut w.dns).hint_text("es. 192.168.1.100"));
                ui.end_row();
            }
        });
        if ui.button("✔ Applica").clicked() {
            if kind.has_cli() {
                let mut cmds = vec![format!("hostname {}", w.hostname.trim())];
                if kind == DeviceKind::Switch {
                    cmds.push(if w.gateway.trim().is_empty() {
                        "no ip default-gateway".into()
                    } else {
                        format!("ip default-gateway {}", w.gateway.trim())
                    });
                }
                w.run_ios(&mut self.net, id, &cmds);
            } else {
                let res = (|| -> Result<(), String> {
                    let name = w.hostname.trim();
                    if name.is_empty() {
                        return Err("il nome non può essere vuoto".into());
                    }
                    if self.net.devices.values().any(|d| d.id != id && d.name.eq_ignore_ascii_case(name)) {
                        return Err(format!("esiste già un dispositivo chiamato {name}"));
                    }
                    let gw = parse_opt_ip(&w.gateway, "gateway")?;
                    let dns = parse_opt_ip(&w.dns, "server DNS")?;
                    let d = self.net.device_mut(id).unwrap();
                    d.name = name.to_string();
                    d.gateway = gw;
                    d.dns_server = dns;
                    Ok(())
                })();
                w.message = Some(match res {
                    Ok(()) => (true, "Configurazione applicata".into()),
                    Err(e) => (false, e),
                });
            }
            self.dirty = true;
            w.load(&self.net, id);
        }
    }

    fn cfg_routing(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        ui.label(RichText::new("Rotte statiche").strong().size(15.0));
        egui::Grid::new(("route", id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Rete:");
            ui.add(egui::TextEdit::singleline(&mut w.route_net).hint_text("192.168.2.0"));
            ui.end_row();
            ui.label("Mask:");
            ui.add(egui::TextEdit::singleline(&mut w.route_mask).hint_text("255.255.255.0"));
            ui.end_row();
            ui.label("Next hop:");
            ui.add(egui::TextEdit::singleline(&mut w.route_nh).hint_text("10.0.0.2"));
            ui.end_row();
        });
        if ui.button("➕ Aggiungi").clicked() {
            let cmd = format!("ip route {} {} {}", w.route_net.trim(), w.route_mask.trim(), w.route_nh.trim());
            if w.run_ios(&mut self.net, id, &[cmd]) {
                w.route_net.clear();
                w.route_mask.clear();
                w.route_nh.clear();
                self.dirty = true;
            }
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Rotte configurate").strong());
        let routes = self.net.devices[&id].static_routes.clone();
        if routes.is_empty() {
            ui.label("nessuna");
        }
        for r in routes {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}  via {}", r.network, r.next_hop)).monospace());
                if ui.small_button("Rimuovi").clicked() {
                    let cmd = format!("no ip route {} {} {}", r.network.network(), r.network.mask(), r.next_hop);
                    w.run_ios(&mut self.net, id, &[cmd]);
                    self.dirty = true;
                }
            });
        }
    }

    fn cfg_vlans(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        ui.label(RichText::new("Database VLAN").strong().size(15.0));
        ui.horizontal(|ui| {
            ui.label("Numero:");
            ui.add(egui::TextEdit::singleline(&mut w.vlan_id).desired_width(60.0));
            ui.label("Nome:");
            ui.add(egui::TextEdit::singleline(&mut w.vlan_name).desired_width(140.0));
            if ui.button("➕ Aggiungi").clicked() {
                let mut cmds = vec![format!("vlan {}", w.vlan_id.trim())];
                if !w.vlan_name.trim().is_empty() {
                    cmds.push(format!("name {}", w.vlan_name.trim().replace(' ', "_")));
                }
                cmds.push("exit".into());
                if w.run_ios(&mut self.net, id, &cmds) {
                    w.vlan_id.clear();
                    w.vlan_name.clear();
                    self.dirty = true;
                }
            }
        });
        ui.add_space(6.0);
        let vlans = self.net.devices[&id].vlans.clone();
        egui::Grid::new(("vlist", id)).striped(true).num_columns(3).show(ui, |ui| {
            for (v, name) in vlans {
                ui.label(v.to_string());
                ui.label(name);
                if v != 1 && ui.small_button("Rimuovi").clicked() {
                    w.run_ios(&mut self.net, id, &[format!("no vlan {v}")]);
                    self.dirty = true;
                }
                ui.end_row();
            }
        });
    }

    fn cfg_iface(&mut self, ui: &mut egui::Ui, id: DeviceId, i: usize, w: &mut DevWin) {
        let d = self.net.devices[&id].clone();
        let ifc = d.interfaces[i].clone();
        let ep = Endpoint { device: id, iface: i };
        let (st, proto) = self.net.port_status(ep);
        ui.label(RichText::new(&ifc.name).strong().size(15.0));
        ui.horizontal(|ui| {
            ui.label("Stato:");
            ui.colored_label(if proto == "up" { LINK_UP } else { LINK_DOWN }, format!("{st} / {proto}"));
        });
        if ifc.is_physical() {
            let peer = self
                .net
                .peer_of(ep)
                .map(|(pd, pi)| self.net.endpoint_label(Endpoint { device: pd, iface: pi }))
                .unwrap_or_else(|| "non collegata".into());
            ui.label(format!("Collegata a: {peer}"));
        }
        ui.label(RichText::new(format!("MAC: {}", ifc.mac)).monospace());
        let mut on = ifc.enabled;
        if ui.checkbox(&mut on, "Porta accesa (Port Status: On)").changed() {
            if d.kind.has_cli() {
                let cmds = vec![format!("interface {}", ifc.name), if on { "no shutdown".into() } else { "shutdown".into() }, "exit".into()];
                w.run_ios(&mut self.net, id, &cmds);
            } else {
                self.net.device_mut(id).unwrap().interfaces[i].enabled = on;
            }
            self.dirty = true;
        }
        ui.add_space(6.0);

        let l3 = !(d.kind.is_l2() && ifc.is_physical()) && d.kind.has_ip();
        if d.kind.is_host() {
            ui.label("L'indirizzo IP dei PC si configura anche da Desktop → Configurazione IP.");
        }
        egui::Grid::new(("ifc", id, i)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            if d.kind.has_cli() {
                ui.label("Descrizione:");
                ui.text_edit_singleline(&mut w.if_desc);
                ui.end_row();
            }
            if l3 {
                ui.label("Indirizzo IP:");
                ui.add(egui::TextEdit::singleline(&mut w.if_ip).hint_text("es. 192.168.1.1"));
                ui.end_row();
                ui.label("Subnet mask:");
                ui.add(egui::TextEdit::singleline(&mut w.if_mask).hint_text("es. 255.255.255.0"));
                ui.end_row();
            }
            if d.kind.is_router() {
                ui.label("IP helper (relay DHCP):");
                ui.add(egui::TextEdit::singleline(&mut w.if_helper).hint_text("facoltativo"));
                ui.end_row();
            }
            if let IfaceKind::Sub { vlan, .. } = ifc.kind {
                ui.label("VLAN (dot1Q):");
                ui.label(if vlan == 0 { "non impostata: usa la CLI 'encapsulation dot1Q N'".into() } else { vlan.to_string() });
                ui.end_row();
            }
        });
        if d.kind.is_l2() && ifc.is_physical() && d.kind == DeviceKind::Switch {
            ui.add_space(4.0);
            let mut trunk = ifc.switchport.mode == PortMode::Trunk;
            ui.horizontal(|ui| {
                ui.label("Modalità:");
                if ui.radio_value(&mut trunk, false, "Access").changed() | ui.radio_value(&mut trunk, true, "Trunk").changed() {
                    let cmds = vec![
                        format!("interface {}", ifc.name),
                        format!("switchport mode {}", if trunk { "trunk" } else { "access" }),
                        "exit".into(),
                    ];
                    w.run_ios(&mut self.net, id, &cmds);
                    self.dirty = true;
                }
            });
            egui::Grid::new(("sp", id, i)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                if !trunk {
                    ui.label("VLAN di accesso:");
                    egui::ComboBox::from_id_salt(("accvlan", id, i)).selected_text(w.if_vlan.clone()).show_ui(ui, |ui| {
                        for (v, name) in &d.vlans {
                            ui.selectable_value(&mut w.if_vlan, v.to_string(), format!("{v} {name}"));
                        }
                    });
                    ui.end_row();
                } else {
                    ui.label("VLAN nativa:");
                    ui.add(egui::TextEdit::singleline(&mut w.if_native).desired_width(60.0));
                    ui.end_row();
                    ui.label("VLAN permesse:");
                    ui.add(egui::TextEdit::singleline(&mut w.if_allowed).hint_text("all oppure 1,10,20-30"));
                    ui.end_row();
                }
            });
        }
        if ui.button("✔ Applica").clicked() {
            if d.kind.has_cli() {
                let mut cmds = vec![format!("interface {}", ifc.name)];
                cmds.push(if w.if_desc.trim().is_empty() { "no description".into() } else { format!("description {}", w.if_desc.trim()) });
                if l3 {
                    if w.if_ip.trim().is_empty() {
                        cmds.push("no ip address".into());
                    } else {
                        let mask = if w.if_mask.trim().is_empty() {
                            npt_core::addr::prefix_to_mask(w.if_ip.trim().parse().map(default_prefix_for).unwrap_or(24)).to_string()
                        } else {
                            w.if_mask.trim().to_string()
                        };
                        let mask = mask.parse::<u32>().map(|m| Ipv4Addr::from(m).to_string()).unwrap_or(mask);
                        cmds.push(format!("ip address {} {}", w.if_ip.trim(), mask));
                    }
                }
                if d.kind.is_router() {
                    cmds.push(if w.if_helper.trim().is_empty() {
                        "no ip helper-address".into()
                    } else {
                        format!("ip helper-address {}", w.if_helper.trim())
                    });
                }
                if d.kind == DeviceKind::Switch && ifc.is_physical() {
                    if ifc.switchport.mode == PortMode::Access {
                        cmds.push(format!("switchport access vlan {}", w.if_vlan.trim()));
                    } else {
                        cmds.push(format!("switchport trunk native vlan {}", w.if_native.trim()));
                        cmds.push(format!("switchport trunk allowed vlan {}", w.if_allowed.trim()));
                    }
                }
                cmds.push("exit".into());
                w.run_ios(&mut self.net, id, &cmds);
            } else {
                let r = apply_host_ip(&mut self.net, id, i, &w.if_ip, &w.if_mask);
                w.message = Some(match r {
                    Ok(()) => (true, "Configurazione applicata".into()),
                    Err(e) => (false, e),
                });
            }
            self.dirty = true;
            w.load(&self.net, id);
        }
    }

    // ------------------------------------------------------------------
    // CLI
    // ------------------------------------------------------------------

    fn cli_tab(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        let prompt = w.cli.prompt(&self.net, id);
        let mut submitted = None;
        console_ui(ui, ("cli", id), &mut w.cli_con, &prompt, &mut submitted);
        if let Some(line) = submitted {
            let echo = if w.cli.mode == CliMode::EnablePassword { String::new() } else { line.clone() };
            let out = w.cli.execute(&mut self.net, id, &line);
            w.cli_con.log.push_str(&format!("{prompt}{echo}\n"));
            if !out.is_empty() {
                w.cli_con.log.push_str(&out);
                if !out.ends_with('\n') {
                    w.cli_con.log.push('\n');
                }
            }
            self.dirty = true;
        }
        ui.horizontal(|ui| {
            if ui.small_button("Pulisci").clicked() {
                w.cli_con.log.clear();
            }
            if ui.small_button("Copia tutto").clicked() {
                ui.ctx().copy_text(w.cli_con.log.clone());
            }
            ui.label(RichText::new("↑/↓ cronologia · ? help · abbreviazioni ammesse (conf t, int g0/0, sh ip int br)").small().weak());
        });
    }

    // ------------------------------------------------------------------
    // Desktop (PC, Laptop, Server)
    // ------------------------------------------------------------------

    fn desktop_tab(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        if w.desktop != DesktopApp::Home {
            if ui.button("⬅ Desktop").clicked() {
                w.desktop = DesktopApp::Home;
            }
            ui.separator();
        }
        match w.desktop {
            DesktopApp::Home => {
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    for (app, icon, label) in [
                        (DesktopApp::IpConfig, "🌐", "Configurazione IP"),
                        (DesktopApp::Prompt, "⌨", "Prompt dei comandi"),
                    ] {
                        let b = egui::Button::new(RichText::new(format!("{icon}\n{label}")).size(15.0)).min_size(Vec2::new(150.0, 90.0));
                        if ui.add(b).clicked() {
                            w.desktop = app;
                            w.host_con.focus = true;
                            w.loaded = None;
                        }
                    }
                });
            }
            DesktopApp::IpConfig => self.ip_config_app(ui, id, w),
            DesktopApp::Prompt => {
                let mut submitted = None;
                console_ui(ui, ("host", id), &mut w.host_con, HOST_PROMPT, &mut submitted);
                if let Some(line) = submitted {
                    w.host_con.log.push_str(&format!("{HOST_PROMPT}{line}\n"));
                    if matches!(line.trim().to_ascii_lowercase().as_str(), "cls" | "clear") {
                        w.host_con.log.clear();
                    } else {
                        let out = run_host_command(&mut self.net, id, &line);
                        w.host_con.log.push_str(&out);
                        w.host_con.log.push_str("\n\n");
                    }
                    self.dirty = true;
                }
            }
        }
    }

    fn ip_config_app(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        if w.loaded.is_none() {
            w.section = Section::Iface(0);
            w.load(&self.net, id);
        }
        ui.label(RichText::new("Configurazione IP — FastEthernet0").strong().size(15.0));
        ui.horizontal(|ui| {
            ui.radio_value(&mut w.ipc_dhcp, true, "DHCP");
            ui.radio_value(&mut w.ipc_dhcp, false, "Statico");
        });
        if w.ipc_dhcp {
            if ui.button("🔄 Richiedi indirizzo DHCP").clicked() {
                self.net.dhcp_request(id, 0);
                if self.mode == Mode::Realtime {
                    self.net.run_until_idle(REALTIME_MAX_TICKS);
                    let ip = self.net.devices[&id].interfaces[0].ip;
                    w.message = Some(match ip {
                        Some(c) if !c.addr.is_link_local() => (true, format!("Richiesta DHCP riuscita: {c}")),
                        _ => (false, "Richiesta DHCP fallita: nessun server ha risposto (indirizzo APIPA)".into()),
                    });
                } else {
                    w.message = Some((true, "DISCOVER inviato: segui i pacchetti nella simulazione".into()));
                }
                self.dirty = true;
                w.loaded = Some(Section::Iface(0));
                let sec = w.section;
                w.section = Section::Iface(0);
                w.load(&self.net, id);
                w.section = sec;
            }
        }
        egui::Grid::new(("ipc", id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            let ro = w.ipc_dhcp;
            for (label, val) in [
                ("Indirizzo IPv4:", &mut w.if_ip),
                ("Subnet mask:", &mut w.if_mask),
                ("Default gateway:", &mut w.gateway),
                ("Server DNS:", &mut w.dns),
            ] {
                ui.label(label);
                ui.add_enabled(!ro, egui::TextEdit::singleline(val));
                ui.end_row();
            }
        });
        if !w.ipc_dhcp && ui.button("✔ Applica").clicked() {
            let r = (|| -> Result<(), String> {
                apply_host_ip(&mut self.net, id, 0, &w.if_ip, &w.if_mask)?;
                let gw = parse_opt_ip(&w.gateway, "gateway")?;
                let dns = parse_opt_ip(&w.dns, "server DNS")?;
                let d = self.net.device_mut(id).unwrap();
                d.gateway = gw;
                d.dns_server = dns;
                d.interfaces[0].dhcp_client = false;
                if let (Some(g), Some(ip)) = (gw, d.interfaces[0].ip) {
                    if !ip.contains(g) {
                        return Err(format!("Attenzione: il gateway {g} non è nella rete {}", ip.as_network()));
                    }
                }
                Ok(())
            })();
            w.message = Some(match r {
                Ok(()) => (true, "Configurazione applicata".into()),
                Err(e) => (false, e),
            });
            self.dirty = true;
        }
        if let Some((ok, m)) = &w.message {
            ui.colored_label(if *ok { LINK_UP } else { LINK_DOWN }, m);
        }
    }

    // ------------------------------------------------------------------
    // Servizi del server
    // ------------------------------------------------------------------

    fn services_tab(&mut self, ui: &mut egui::Ui, id: DeviceId, w: &mut DevWin) {
        egui::ScrollArea::vertical().id_salt(("svc", id)).show(ui, |ui| {
            // --- DHCP ---
            ui.label(RichText::new("DHCP").strong().size(15.0));
            let mut on = self.net.devices[&id].services.dhcp;
            if ui.checkbox(&mut on, "Servizio attivo").changed() {
                self.net.device_mut(id).unwrap().services.dhcp = on;
                self.dirty = true;
            }
            egui::Grid::new(("poolf", id)).num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Nome pool:");
                ui.text_edit_singleline(&mut w.pool_name);
                ui.label("Gateway:");
                ui.text_edit_singleline(&mut w.pool_gw);
                ui.end_row();
                ui.label("Server DNS:");
                ui.text_edit_singleline(&mut w.pool_dns);
                ui.label("IP iniziale:");
                ui.text_edit_singleline(&mut w.pool_start);
                ui.end_row();
                ui.label("Subnet mask:");
                ui.text_edit_singleline(&mut w.pool_mask);
                ui.label("Max utenti:");
                ui.text_edit_singleline(&mut w.pool_max);
                ui.end_row();
            });
            let mut action = None;
            ui.horizontal(|ui| {
                if ui.button("➕ Aggiungi").clicked() {
                    action = Some(false);
                }
                if w.pool_sel.is_some() && ui.button("💾 Salva modifiche").clicked() {
                    action = Some(true);
                }
                if let Some(sel) = w.pool_sel {
                    if ui.button("🗑 Rimuovi").clicked() {
                        let d = self.net.device_mut(id).unwrap();
                        if sel < d.dhcp_pools.len() {
                            d.dhcp_pools.remove(sel);
                        }
                        w.pool_sel = None;
                        self.dirty = true;
                    }
                }
            });
            if let Some(replace) = action {
                match build_pool(w) {
                    Ok(pool) => {
                        let d = self.net.device_mut(id).unwrap();
                        match (replace, w.pool_sel) {
                            (true, Some(s)) if s < d.dhcp_pools.len() => d.dhcp_pools[s] = pool,
                            _ => {
                                if d.dhcp_pools.iter().any(|p| p.name == pool.name) {
                                    w.message = Some((false, format!("esiste già un pool chiamato {}", pool.name)));
                                } else {
                                    d.dhcp_pools.push(pool);
                                    w.message = Some((true, "Pool aggiunto".into()));
                                }
                            }
                        }
                        self.dirty = true;
                    }
                    Err(e) => w.message = Some((false, e)),
                }
            }
            let pools = self.net.devices[&id].dhcp_pools.clone();
            egui::Grid::new(("pools", id)).striped(true).num_columns(6).show(ui, |ui| {
                for h in ["Pool", "Gateway", "DNS", "IP iniziale", "Rete", "Max"] {
                    ui.label(RichText::new(h).small().strong());
                }
                ui.end_row();
                for (i, p) in pools.iter().enumerate() {
                    if ui.selectable_label(w.pool_sel == Some(i), &p.name).clicked() {
                        w.pool_sel = Some(i);
                        w.pool_name = p.name.clone();
                        w.pool_gw = p.default_router.map(|x| x.to_string()).unwrap_or_default();
                        w.pool_dns = p.dns.map(|x| x.to_string()).unwrap_or_default();
                        w.pool_start = p.start.map(|x| x.to_string()).unwrap_or_default();
                        w.pool_mask = p.network.map(|n| n.mask().to_string()).unwrap_or_default();
                        w.pool_max = p.max_users.map(|x| x.to_string()).unwrap_or_default();
                    }
                    ui.label(p.default_router.map(|x| x.to_string()).unwrap_or("-".into()));
                    ui.label(p.dns.map(|x| x.to_string()).unwrap_or("-".into()));
                    ui.label(p.start.map(|x| x.to_string()).unwrap_or("-".into()));
                    ui.label(p.network.map(|x| x.to_string()).unwrap_or("-".into()));
                    ui.label(p.max_users.map(|x| x.to_string()).unwrap_or("-".into()));
                    ui.end_row();
                }
            });
            ui.add_space(4.0);
            ui.label(RichText::new("Il server deve avere un IP nella rete del pool (oppure il router deve avere 'ip helper-address').").small().weak());

            ui.separator();
            // --- DNS ---
            ui.label(RichText::new("DNS").strong().size(15.0));
            let mut on = self.net.devices[&id].services.dns;
            if ui.checkbox(&mut on, "Servizio attivo").changed() {
                self.net.device_mut(id).unwrap().services.dns = on;
                self.dirty = true;
            }
            ui.horizontal(|ui| {
                ui.label("Nome:");
                ui.add(egui::TextEdit::singleline(&mut w.dns_name).hint_text("www.scuola.it").desired_width(150.0));
                ui.label("Indirizzo:");
                ui.add(egui::TextEdit::singleline(&mut w.dns_addr).hint_text("192.168.1.100").desired_width(120.0));
                if ui.button("➕ Aggiungi").clicked() {
                    match w.dns_addr.trim().parse::<Ipv4Addr>() {
                        Ok(ip) if !w.dns_name.trim().is_empty() => {
                            let d = self.net.device_mut(id).unwrap();
                            let name = w.dns_name.trim().to_string();
                            d.dns_records.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                            d.dns_records.push((name, ip));
                            w.dns_name.clear();
                            w.dns_addr.clear();
                            self.dirty = true;
                        }
                        _ => w.message = Some((false, "Inserisci un nome e un indirizzo IP valido".into())),
                    }
                }
            });
            let recs = self.net.devices[&id].dns_records.clone();
            for (n, ip) in recs {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{n:<28} A  {ip}")).monospace());
                    if ui.small_button("✖").clicked() {
                        self.net.device_mut(id).unwrap().dns_records.retain(|(x, _)| *x != n);
                        self.dirty = true;
                    }
                });
            }
            if let Some((ok, m)) = &w.message {
                ui.colored_label(if *ok { LINK_UP } else { LINK_DOWN }, m);
            }
        });
    }
}

// ----------------------------------------------------------------------
// Utilità
// ----------------------------------------------------------------------

/// Console testuale con riga di input, cronologia e scorrimento automatico.
fn console_ui(ui: &mut egui::Ui, salt: (&str, DeviceId), con: &mut Console, prompt: &str, submitted: &mut Option<String>) {
    let bg = Color32::from_rgb(18, 22, 28);
    let fg = Color32::from_rgb(220, 226, 232);
    let font = FontId::monospace(13.0);
    let h = (ui.available_height() - 30.0).max(120.0);
    egui::Frame::new().fill(bg).inner_margin(8.0).corner_radius(4.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::both().id_salt((salt.0, salt.1, "log")).max_height(h - 40.0).stick_to_bottom(true).auto_shrink([false, false]).show(
            ui,
            |ui| {
                // Niente a capo automatico: le tabelle dei comandi show restano allineate.
                ui.add(egui::Label::new(RichText::new(&con.log).font(font.clone()).color(fg)).wrap_mode(egui::TextWrapMode::Extend));
            },
        );
        ui.horizontal(|ui| {
            ui.label(RichText::new(prompt).font(font.clone()).color(Color32::from_rgb(130, 200, 255)));
            let id = egui::Id::unique((salt.0, salt.1, "input"));
            let resp = ui.add(
                egui::TextEdit::singleline(&mut con.input)
                    .id(id)
                    .font(font.clone())
                    .text_color(fg)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY),
            );
            if con.focus {
                resp.request_focus();
                con.focus = false;
            }
            if resp.has_focus() {
                let (up, down) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
                if up && !con.history.is_empty() {
                    let p = con.hist_pos.map(|p| p.saturating_sub(1)).unwrap_or(con.history.len() - 1);
                    con.hist_pos = Some(p);
                    con.input = con.history[p].clone();
                }
                if down {
                    if let Some(p) = con.hist_pos {
                        if p + 1 < con.history.len() {
                            con.hist_pos = Some(p + 1);
                            con.input = con.history[p + 1].clone();
                        } else {
                            con.hist_pos = None;
                            con.input.clear();
                        }
                    }
                }
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let line = std::mem::take(&mut con.input);
                if !line.trim().is_empty() {
                    con.history.push(line.clone());
                }
                con.hist_pos = None;
                *submitted = Some(line);
                resp.request_focus();
            }
        });
    });
}

fn parse_opt_ip(s: &str, what: &str) -> Result<Option<Ipv4Addr>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    s.parse().map(Some).map_err(|_| format!("{what} non valido: {s}"))
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

fn apply_host_ip(net: &mut Network, id: DeviceId, i: usize, ip: &str, mask: &str) -> Result<(), String> {
    let ip = ip.trim();
    if ip.is_empty() {
        net.device_mut(id).unwrap().interfaces[i].ip = None;
        return Ok(());
    }
    let addr: Ipv4Addr = ip.parse().map_err(|_| format!("IP non valido: {ip}"))?;
    let cidr = Ipv4Cidr::new(addr, parse_mask(mask, addr)?);
    validate_host_ip(cidr)?;
    if let Some(d) = net.devices.values().find(|d| d.id != id && d.interfaces.iter().any(|x| x.ip.map(|c| c.addr) == Some(addr))) {
        return Err(format!("{addr} è già usato da {}", d.name));
    }
    let d = net.device_mut(id).unwrap();
    d.interfaces[i].ip = Some(cidr);
    d.interfaces[i].dhcp_client = false;
    Ok(())
}

fn build_pool(w: &DevWin) -> Result<DhcpPool, String> {
    let name = w.pool_name.trim();
    if name.is_empty() {
        return Err("il pool deve avere un nome".into());
    }
    let start: Ipv4Addr = w.pool_start.trim().parse().map_err(|_| "IP iniziale non valido".to_string())?;
    let prefix = parse_mask(&w.pool_mask, start)?;
    let mut p = DhcpPool::new(name);
    p.network = Some(Ipv4Cidr::new(start, prefix).as_network());
    p.start = Some(start);
    p.default_router = parse_opt_ip(&w.pool_gw, "gateway")?;
    p.dns = parse_opt_ip(&w.pool_dns, "DNS")?;
    p.max_users = match w.pool_max.trim() {
        "" => None,
        m => Some(m.parse().map_err(|_| "numero massimo di utenti non valido".to_string())?),
    };
    Ok(p)
}
