//! Riga di comando in stile IOS per router e switch.
//!
//! Supporta le abbreviazioni (`conf t`, `int g0/0`, `sh ip int br`), l'help
//! contestuale con `?`, `do` nelle modalità di configurazione e `no` per
//! annullare i comandi.

use crate::addr::{mask_to_prefix, Ipv4Cidr};
use crate::device::{
    expand_iface_name, format_vlan_list, parse_vlan_list, short_iface_name, DeviceId, DhcpPool, IfaceKind, PortMode,
    RouteKind, StaticRoute,
};
use crate::sim::{validate_host_ip, Endpoint, Network, PingStatus, REALTIME_MAX_TICKS};
use std::fmt::Write as _;
use std::net::Ipv4Addr;

#[derive(Clone, Debug, PartialEq)]
pub enum CliMode {
    User,
    Enable,
    Config,
    Iface(Vec<usize>),
    Vlan(Vec<u16>),
    DhcpPool(String),
    Line,
    /// In attesa della password di `enable`.
    EnablePassword,
}

#[derive(Clone, Debug)]
pub struct CliSession {
    pub mode: CliMode,
    pub history: Vec<String>,
}

impl Default for CliSession {
    fn default() -> Self {
        Self::new()
    }
}

const INVALID: &str = "% Invalid input detected.";
const INCOMPLETE: &str = "% Incomplete command.";

type Res = Result<String, String>;

/// Sceglie la parola chiave che inizia con `tok` (abbreviazioni univoche).
fn pick<'a>(tok: &str, options: &[&'a str]) -> Result<&'a str, String> {
    let t = tok.to_ascii_lowercase();
    if let Some(o) = options.iter().find(|o| **o == t) {
        return Ok(o);
    }
    let m: Vec<&&str> = options.iter().filter(|o| o.starts_with(&t)).collect();
    match m.len() {
        0 => Err(INVALID.into()),
        1 => Ok(m[0]),
        _ => Err(format!("% Ambiguous command: \"{tok}\"")),
    }
}

fn arg<'a>(t: &[&'a str], i: usize) -> Result<&'a str, String> {
    t.get(i).copied().ok_or_else(|| INCOMPLETE.to_string())
}

fn parse_ip(s: &str) -> Result<Ipv4Addr, String> {
    s.parse().map_err(|_| format!("% Invalid IP address: {s}"))
}

/// Accetta una maschera puntata o `/nn`.
fn parse_mask(s: &str) -> Result<u8, String> {
    if let Some(p) = s.strip_prefix('/') {
        return p.parse::<u8>().ok().filter(|p| *p <= 32).ok_or_else(|| format!("% Invalid mask: {s}"));
    }
    mask_to_prefix(parse_ip(s)?).map_err(|_| format!("% Invalid mask: {s}"))
}

fn simple_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("$npt${h:016x}")
}

impl CliSession {
    pub fn new() -> Self {
        Self { mode: CliMode::User, history: Vec::new() }
    }

    /// Testo mostrato all'apertura della console.
    pub fn greeting(&self, net: &Network, dev: DeviceId) -> String {
        let d = &net.devices[&dev];
        let mut s = String::new();
        if let Some(b) = &d.banner {
            s.push_str(b);
            s.push('\n');
        }
        s.push_str(&format!("{} — console di {} ({})\nDigita ? per l'elenco dei comandi.\n", d.kind.model(), d.name, d.kind.label()));
        s
    }

    pub fn prompt(&self, net: &Network, dev: DeviceId) -> String {
        let d = &net.devices[&dev];
        let name = &d.name;
        match &self.mode {
            CliMode::User => format!("{name}>"),
            CliMode::Enable => format!("{name}#"),
            CliMode::Config => format!("{name}(config)#"),
            CliMode::Iface(v) if v.len() > 1 => format!("{name}(config-if-range)#"),
            CliMode::Iface(v) => match v.first().map(|i| d.interfaces[*i].kind) {
                Some(IfaceKind::Sub { .. }) => format!("{name}(config-subif)#"),
                _ => format!("{name}(config-if)#"),
            },
            CliMode::Vlan(_) => format!("{name}(config-vlan)#"),
            CliMode::DhcpPool(_) => format!("{name}(dhcp-config)#"),
            CliMode::Line => format!("{name}(config-line)#"),
            CliMode::EnablePassword => "Password: ".into(),
        }
    }

    /// Esegue una riga e restituisce l'output da mostrare.
    pub fn execute(&mut self, net: &mut Network, dev: DeviceId, line: &str) -> String {
        if self.mode == CliMode::EnablePassword {
            let ok = net.devices[&dev].enable_secret.as_deref() == Some(line.trim());
            self.mode = if ok { CliMode::Enable } else { CliMode::User };
            return if ok { String::new() } else { "% Bad secret".into() };
        }
        let line = line.trim();
        if line.is_empty() {
            return String::new();
        }
        self.history.push(line.to_string());
        if let Some(prefix) = line.strip_suffix('?') {
            return self.help(net, dev, prefix.trim());
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        match self.run(net, dev, &toks) {
            Ok(s) | Err(s) => s,
        }
    }

    fn run(&mut self, net: &mut Network, dev: DeviceId, t: &[&str]) -> Res {
        match self.mode.clone() {
            CliMode::User => self.user_cmd(net, dev, t),
            CliMode::Enable => self.enable_cmd(net, dev, t),
            CliMode::EnablePassword => Ok(String::new()),
            mode => {
                let first = t[0].to_ascii_lowercase();
                if "do".starts_with(&first) && first.len() >= 2 && t.len() > 1 {
                    return self.enable_cmd(net, dev, &t[1..]);
                }
                if let Ok(k) = pick(&first, &["end", "exit"]) {
                    if t.len() == 1 {
                        self.mode = match (k, &mode) {
                            ("end", _) | (_, CliMode::Config) => CliMode::Enable,
                            _ => CliMode::Config,
                        };
                        return Ok(String::new());
                    }
                }
                let sub = match &mode {
                    CliMode::Iface(ifs) => Some(self.iface_cmd(net, dev, ifs.clone(), t)),
                    CliMode::Vlan(v) => Some(self.vlan_cmd(net, dev, v.clone(), t)),
                    CliMode::DhcpPool(p) => Some(self.dhcp_cmd(net, dev, p.clone(), t)),
                    CliMode::Line => Some(self.line_cmd(t)),
                    _ => None,
                };
                match sub {
                    // Come su IOS, i comandi globali funzionano anche nelle sottomodalità.
                    Some(Err(e)) if e == INVALID => self.config_cmd(net, dev, t),
                    Some(r) => r,
                    None => self.config_cmd(net, dev, t),
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Modalità utente e privilegiata
    // ------------------------------------------------------------------

    fn user_cmd(&mut self, net: &mut Network, dev: DeviceId, t: &[&str]) -> Res {
        match pick(t[0], &["enable", "exit", "logout", "ping", "traceroute", "show"])? {
            "enable" => {
                if net.devices[&dev].enable_secret.is_some() {
                    self.mode = CliMode::EnablePassword;
                } else {
                    self.mode = CliMode::Enable;
                }
                Ok(String::new())
            }
            "exit" | "logout" => Ok("\nSessione chiusa. Premi INVIO per ricominciare.".into()),
            "ping" => self.ping(net, dev, t),
            "traceroute" => self.traceroute(net, dev, t),
            "show" => self.show(net, dev, &t[1..]),
            _ => Err(INVALID.into()),
        }
    }

    fn enable_cmd(&mut self, net: &mut Network, dev: DeviceId, t: &[&str]) -> Res {
        let cmds = ["configure", "disable", "exit", "logout", "show", "ping", "traceroute", "copy", "write", "clear", "reload", "enable"];
        match pick(t[0], &cmds)? {
            "configure" => {
                if let Some(x) = t.get(1) {
                    pick(x, &["terminal"])?;
                }
                self.mode = CliMode::Config;
                Ok("Enter configuration commands, one per line.  End with CNTL/Z.".into())
            }
            "disable" | "exit" | "logout" => {
                self.mode = CliMode::User;
                Ok(String::new())
            }
            "enable" => Ok(String::new()),
            "show" => self.show(net, dev, &t[1..]),
            "ping" => self.ping(net, dev, t),
            "traceroute" => self.traceroute(net, dev, t),
            "copy" | "write" => Ok("Building configuration...\n[OK]".into()),
            "reload" => {
                net.devices.get_mut(&dev).unwrap().state = Default::default();
                self.mode = CliMode::User;
                Ok("Riavvio del dispositivo... tabelle dinamiche azzerate.".into())
            }
            "clear" => {
                let what = pick(arg(t, 1)?, &["mac", "arp-cache", "ip"])?;
                let d = net.devices.get_mut(&dev).unwrap();
                match what {
                    "mac" => d.state.mac_table.clear(),
                    _ => d.state.arp_table.clear(),
                }
                Ok(String::new())
            }
            _ => Err(INVALID.into()),
        }
    }

    fn ping(&mut self, net: &mut Network, dev: DeviceId, t: &[&str]) -> Res {
        let target = arg(t, 1)?;
        let dst = net.resolve_blocking(dev, target).map_err(|e| format!("% Unrecognized host or address: {e}"))?;
        let mut out = format!("Type escape sequence to abort.\nSending 5, 100-byte ICMP Echos to {dst}, timeout is 2 seconds:\n");
        let mut ok = 0;
        for _ in 0..5 {
            let idx = net.ping_ttl(dev, dst, None)?;
            net.run_until_idle(REALTIME_MAX_TICKS);
            out.push(match &net.pings[idx].status {
                PingStatus::Success { .. } => {
                    ok += 1;
                    '!'
                }
                PingStatus::Failed(r) if r.contains("unreachable") => 'U',
                _ => '.',
            });
        }
        let _ = write!(out, "\nSuccess rate is {} percent ({ok}/5)", ok * 20);
        if ok > 0 {
            out.push_str(", round-trip min/avg/max = 0/0/1 ms");
        }
        Ok(out)
    }

    fn traceroute(&mut self, net: &mut Network, dev: DeviceId, t: &[&str]) -> Res {
        let target = arg(t, 1)?;
        let dst = net.resolve_blocking(dev, target).map_err(|e| format!("% Unrecognized host or address: {e}"))?;
        let hops = net.traceroute(dev, dst, 30)?;
        let mut out = format!("Type escape sequence to abort.\nTracing the route to {dst}\n\n");
        for h in hops {
            match h.from {
                Some(ip) => writeln!(out, "  {:<3} {:<16} 0 msec    0 msec    0 msec", h.ttl, ip.to_string()),
                None => writeln!(out, "  {:<3} *    *    *", h.ttl),
            }
            .ok();
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // show
    // ------------------------------------------------------------------

    fn show(&self, net: &Network, dev: DeviceId, t: &[&str]) -> Res {
        let what = pick(
            arg(t, 0)?,
            &["running-config", "startup-config", "ip", "interfaces", "vlan", "mac", "mac-address-table", "arp", "version", "history", "cdp"],
        )?;
        let d = &net.devices[&dev];
        match what {
            "running-config" | "startup-config" => Ok(self.running_config(net, dev)),
            "version" => Ok(format!(
                "{} — sistema operativo virtuale NPT easy {}\n{} è attivo da {} tick\n{} interfacce di rete\nConfiguration register is 0x2102",
                d.kind.model(),
                env!("CARGO_PKG_VERSION"),
                d.name,
                net.tick,
                d.interfaces.iter().filter(|i| i.is_physical()).count()
            )),
            "history" => Ok(self.history.join("\n")),
            "arp" => Ok(show_arp(net, dev)),
            "mac" | "mac-address-table" => Ok(show_mac(net, dev)),
            "vlan" => {
                if !d.kind.is_l2() {
                    return Err(INVALID.into());
                }
                Ok(show_vlan(net, dev))
            }
            "interfaces" => match t.get(1) {
                Some(x) if "trunk".starts_with(&x.to_ascii_lowercase()) => Ok(show_trunk(net, dev)),
                Some(_) => {
                    let name = t[1..].join("");
                    let i = d.iface_by_name(&name).ok_or("% Invalid interface")?;
                    Ok(show_interface(net, dev, i))
                }
                None => Ok((0..d.interfaces.len()).map(|i| show_interface(net, dev, i)).collect::<Vec<_>>().join("\n")),
            },
            "ip" => match pick(arg(t, 1)?, &["interface", "route", "arp", "dhcp"])? {
                "interface" => Ok(show_ip_int_brief(net, dev)),
                "route" => {
                    if d.kind.is_l2() {
                        return Ok(format!("Default gateway is {}", d.gateway.map(|g| g.to_string()).unwrap_or("not set".into())));
                    }
                    Ok(show_ip_route(net, dev))
                }
                "arp" => Ok(show_arp(net, dev)),
                "dhcp" => match pick(arg(t, 2)?, &["binding", "pool"])? {
                    "binding" => Ok(show_dhcp_binding(net, dev)),
                    _ => Ok(show_dhcp_pool(net, dev)),
                },
                _ => Err(INVALID.into()),
            },
            "cdp" => Ok(show_neighbors(net, dev)),
            _ => Err(INVALID.into()),
        }
    }

    fn running_config(&self, net: &Network, dev: DeviceId) -> String {
        let d = &net.devices[&dev];
        let mut s = String::from("Building configuration...\n\nCurrent configuration:\n!\n");
        let _ = writeln!(s, "hostname {}\n!", d.name);
        if let Some(sec) = &d.enable_secret {
            let _ = writeln!(s, "enable secret 5 {}\n!", simple_hash(sec));
        }
        for (a, b) in &d.dhcp_excluded {
            let _ = writeln!(s, "ip dhcp excluded-address {a} {b}");
        }
        for p in &d.dhcp_pools {
            let _ = writeln!(s, "!\nip dhcp pool {}", p.name);
            if let Some(n) = p.network {
                let _ = writeln!(s, " network {} {}", n.network(), n.mask());
            }
            if let Some(r) = p.default_router {
                let _ = writeln!(s, " default-router {r}");
            }
            if let Some(r) = p.dns {
                let _ = writeln!(s, " dns-server {r}");
            }
        }
        if !d.dhcp_pools.is_empty() || !d.dhcp_excluded.is_empty() {
            s.push_str("!\n");
        }
        if d.kind.is_l2() {
            for (id, name) in &d.vlans {
                if *id != 1 {
                    let _ = writeln!(s, "vlan {id}\n name {name}\n!");
                }
            }
        }
        for ifc in &d.interfaces {
            let _ = writeln!(s, "interface {}", ifc.name);
            if !ifc.description.is_empty() {
                let _ = writeln!(s, " description {}", ifc.description);
            }
            if let IfaceKind::Sub { vlan, .. } = ifc.kind {
                if vlan != 0 {
                    let _ = writeln!(s, " encapsulation dot1Q {vlan}");
                }
            }
            if d.kind.is_l2() && ifc.is_physical() {
                let sp = &ifc.switchport;
                match sp.mode {
                    PortMode::Access => {
                        if sp.access_vlan != 1 {
                            let _ = writeln!(s, " switchport access vlan {}", sp.access_vlan);
                        }
                        s.push_str(" switchport mode access\n");
                    }
                    PortMode::Trunk => {
                        if sp.native_vlan != 1 {
                            let _ = writeln!(s, " switchport trunk native vlan {}", sp.native_vlan);
                        }
                        if let Some(a) = &sp.allowed {
                            let _ = writeln!(s, " switchport trunk allowed vlan {}", format_vlan_list(a));
                        }
                        s.push_str(" switchport mode trunk\n");
                    }
                }
            }
            if ifc.dhcp_client {
                s.push_str(" ip address dhcp\n");
            } else if let Some(ip) = ifc.ip {
                let _ = writeln!(s, " ip address {} {}", ip.addr, ip.mask());
            } else if !d.kind.is_l2() || !ifc.is_physical() {
                s.push_str(" no ip address\n");
            }
            if let Some(h) = ifc.helper {
                let _ = writeln!(s, " ip helper-address {h}");
            }
            if !ifc.enabled {
                s.push_str(" shutdown\n");
            }
            s.push_str("!\n");
        }
        if d.kind.is_l2() {
            if let Some(g) = d.gateway {
                let _ = writeln!(s, "ip default-gateway {g}\n!");
            }
        }
        for r in &d.static_routes {
            let _ = writeln!(s, "ip route {} {} {}", r.network.network(), r.network.mask(), r.next_hop);
        }
        if !d.static_routes.is_empty() {
            s.push_str("!\n");
        }
        if let Some(b) = &d.banner {
            let _ = writeln!(s, "banner motd ^C{b}^C\n!");
        }
        s.push_str("line con 0\n!\nline vty 0 4\n login\n!\nend");
        s
    }

    // ------------------------------------------------------------------
    // Configurazione globale
    // ------------------------------------------------------------------

    fn config_cmd(&mut self, net: &mut Network, dev: DeviceId, t: &[&str]) -> Res {
        let (neg, t) = match pick(t[0], &["no"]) {
            Ok(_) if t[0].len() >= 2 && t.len() > 1 => (true, &t[1..]),
            _ => (false, t),
        };
        let cmds = [
            "hostname", "interface", "ip", "vlan", "enable", "banner", "service", "line", "username", "router", "access-list",
            "spanning-tree", "cdp",
        ];
        let is_l2 = net.devices[&dev].kind.is_l2();
        match pick(t[0], &cmds)? {
            "hostname" => {
                if neg {
                    net.devices.get_mut(&dev).unwrap().name = net.devices[&dev].kind.label().to_string();
                    return Ok(String::new());
                }
                let name = arg(t, 1)?;
                if net.devices.values().any(|d| d.id != dev && d.name.eq_ignore_ascii_case(name)) {
                    return Err(format!("% Il nome {name} è già usato da un altro dispositivo"));
                }
                net.devices.get_mut(&dev).unwrap().name = name.to_string();
                Ok(String::new())
            }
            "interface" => {
                if neg {
                    let name = t[1..].join("");
                    let d = net.devices.get_mut(&dev).unwrap();
                    let i = d.iface_by_name(&name).ok_or("% Invalid interface")?;
                    if d.interfaces[i].is_physical() {
                        return Err("% Non si può rimuovere un'interfaccia fisica".into());
                    }
                    d.interfaces.remove(i);
                    d.state.pending.clear();
                    return Ok(String::new());
                }
                let rest = &t[1..];
                if rest.is_empty() {
                    return Err(INCOMPLETE.into());
                }
                let ifs = if pick(rest[0], &["range"]).is_ok() && rest[0].len() >= 2 {
                    parse_range(net, dev, &rest[1..].join(" "))?
                } else {
                    vec![self.resolve_iface(net, dev, &rest.join(""))?]
                };
                self.mode = CliMode::Iface(ifs);
                Ok(String::new())
            }
            "ip" => self.ip_global(net, dev, &t[1..], neg),
            "vlan" => {
                if !is_l2 {
                    return Err(INVALID.into());
                }
                let ids = parse_vlan_list(arg(t, 1)?).map_err(|e| format!("% {e}"))?;
                let d = net.devices.get_mut(&dev).unwrap();
                if neg {
                    for v in ids {
                        if v == 1 {
                            return Err("% La VLAN 1 non può essere eliminata".into());
                        }
                        d.vlans.remove(&v);
                    }
                    return Ok(String::new());
                }
                for v in &ids {
                    d.vlans.entry(*v).or_insert_with(|| format!("VLAN{v:04}"));
                }
                self.mode = CliMode::Vlan(ids);
                Ok(String::new())
            }
            "enable" => {
                pick(arg(t, 1)?, &["secret", "password"])?;
                let d = net.devices.get_mut(&dev).unwrap();
                if neg {
                    d.enable_secret = None;
                } else {
                    // `enable secret 5 <hash>` non è supportato: si usa il testo in chiaro.
                    let pw = t.last().filter(|_| t.len() >= 3).ok_or(INCOMPLETE)?;
                    d.enable_secret = Some(pw.to_string());
                }
                Ok(String::new())
            }
            "banner" => {
                let d = net.devices.get_mut(&dev).unwrap();
                if neg {
                    d.banner = None;
                    return Ok(String::new());
                }
                pick(arg(t, 1)?, &["motd"])?;
                let text = t[2..].join(" ");
                let delim = text.chars().next().ok_or(INCOMPLETE)?;
                let inner: String = text.chars().skip(1).take_while(|c| *c != delim).collect();
                d.banner = Some(inner);
                Ok(String::new())
            }
            "line" => {
                self.mode = CliMode::Line;
                Ok(String::new())
            }
            // Comandi accettati ma non ancora simulati.
            "service" | "username" | "spanning-tree" | "cdp" => Ok(String::new()),
            "router" | "access-list" => Ok("% Funzione non ancora disponibile in questa versione di NPT easy".into()),
            _ => Err(INVALID.into()),
        }
    }

    /// Trova (o crea, per sottointerfacce e SVI) l'interfaccia indicata.
    fn resolve_iface(&self, net: &mut Network, dev: DeviceId, name: &str) -> Result<usize, String> {
        let full = expand_iface_name(name).ok_or("% Invalid interface")?;
        if let Some(i) = net.devices[&dev].iface_by_name(&full) {
            return Ok(i);
        }
        let kind = net.devices[&dev].kind;
        if let Some(v) = full.strip_prefix("Vlan") {
            if kind != crate::DeviceKind::Switch {
                return Err("% Invalid interface".into());
            }
            let vlan: u16 = v.parse().map_err(|_| "% Invalid interface")?;
            let mac = net.alloc_mac();
            return Ok(net.devices.get_mut(&dev).unwrap().ensure_svi(vlan, mac));
        }
        if let Some((parent, num)) = full.rsplit_once('.') {
            if !kind.is_router() {
                return Err("% Le sottointerfacce esistono solo sui router".into());
            }
            let d = net.devices.get_mut(&dev).unwrap();
            let p = d.iface_by_name(parent).ok_or("% Invalid interface")?;
            let n: u32 = num.parse().map_err(|_| "% Invalid interface")?;
            return Ok(d.ensure_subinterface(p, n));
        }
        Err("% Invalid interface".into())
    }

    fn ip_global(&mut self, net: &mut Network, dev: DeviceId, t: &[&str], neg: bool) -> Res {
        let kind = net.devices[&dev].kind;
        let sub = pick(
            arg(t, 0)?,
            &["route", "default-gateway", "dhcp", "routing", "domain-lookup", "domain-name", "name-server", "ssh", "http"],
        )?;
        let d = net.devices.get_mut(&dev).unwrap();
        match sub {
            "route" => {
                if !kind.is_router() {
                    return Err(INVALID.into());
                }
                let net_ip = parse_ip(arg(t, 1)?)?;
                let prefix = parse_mask(arg(t, 2)?)?;
                let network = Ipv4Cidr::new(net_ip, prefix).as_network();
                if neg {
                    d.static_routes.retain(|r| r.network != network || t.get(3).is_some_and(|nh| r.next_hop.to_string() != *nh));
                    return Ok(String::new());
                }
                let nh_tok = arg(t, 3)?;
                let next_hop = match nh_tok.parse::<Ipv4Addr>() {
                    Ok(ip) => ip,
                    Err(_) => return Err("% Indica il next hop come indirizzo IP (es. ip route 10.0.0.0 255.0.0.0 192.168.1.2)".into()),
                };
                let r = StaticRoute { network, next_hop };
                if !d.static_routes.contains(&r) {
                    d.static_routes.push(r);
                }
                Ok(String::new())
            }
            "default-gateway" => {
                if !kind.is_l2() {
                    return Err(INVALID.into());
                }
                d.gateway = if neg { None } else { Some(parse_ip(arg(t, 1)?)?) };
                Ok(String::new())
            }
            "name-server" => {
                d.dns_server = if neg { None } else { Some(parse_ip(arg(t, 1)?)?) };
                Ok(String::new())
            }
            "dhcp" => {
                if !kind.is_router() {
                    return Err(INVALID.into());
                }
                match pick(arg(t, 1)?, &["pool", "excluded-address"])? {
                    "pool" => {
                        let name = arg(t, 2)?.to_string();
                        if neg {
                            d.dhcp_pools.retain(|p| p.name != name);
                            return Ok(String::new());
                        }
                        if !d.dhcp_pools.iter().any(|p| p.name == name) {
                            d.dhcp_pools.push(DhcpPool::new(&name));
                        }
                        self.mode = CliMode::DhcpPool(name);
                        Ok(String::new())
                    }
                    _ => {
                        let a = parse_ip(arg(t, 2)?)?;
                        let b = match t.get(3) {
                            Some(x) => parse_ip(x)?,
                            None => a,
                        };
                        if neg {
                            d.dhcp_excluded.retain(|r| *r != (a, b));
                        } else if !d.dhcp_excluded.contains(&(a, b)) {
                            d.dhcp_excluded.push((a, b));
                        }
                        Ok(String::new())
                    }
                }
            }
            _ => Ok(String::new()),
        }
    }

    // ------------------------------------------------------------------
    // Configurazione interfaccia
    // ------------------------------------------------------------------

    fn iface_cmd(&mut self, net: &mut Network, dev: DeviceId, ifs: Vec<usize>, t: &[&str]) -> Res {
        let (neg, t) = match pick(t[0], &["no"]) {
            Ok(_) if t[0].len() >= 2 && t.len() > 1 => (true, &t[1..]),
            Ok(_) if t[0].len() >= 2 => return Err(INCOMPLETE.into()),
            _ => (false, t),
        };
        let kind = net.devices[&dev].kind;
        let cmd = pick(
            t[0],
            &["shutdown", "ip", "description", "switchport", "encapsulation", "speed", "duplex", "spanning-tree", "cdp", "mdix"],
        )?;
        let mut out = String::new();
        let mut dhcp_ifaces = Vec::new();
        for &i in &ifs {
            let d = net.devices.get_mut(&dev).unwrap();
            let is_phys = d.interfaces[i].is_physical();
            let is_sub = matches!(d.interfaces[i].kind, IfaceKind::Sub { .. });
            match cmd {
                "shutdown" => d.interfaces[i].enabled = neg,
                "description" => d.interfaces[i].description = if neg { String::new() } else { t[1..].join(" ") },
                "ip" => {
                    if kind.is_l2() && is_phys {
                        return Err(INVALID.into());
                    }
                    match pick(arg(t, 1)?, &["address", "helper-address"])? {
                        "address" if neg => {
                            d.interfaces[i].ip = None;
                            d.interfaces[i].dhcp_client = false;
                        }
                        "address" => {
                            let a = arg(t, 2)?;
                            if pick(a, &["dhcp"]).is_ok() && !a.contains('.') {
                                dhcp_ifaces.push(i);
                                continue;
                            }
                            let cidr = Ipv4Cidr::new(parse_ip(a)?, parse_mask(arg(t, 3)?)?);
                            validate_host_ip(cidr).map_err(|e| format!("% {e}"))?;
                            for (j, other) in d.interfaces.iter().enumerate() {
                                if let Some(o) = other.ip {
                                    if j != i && (o.contains(cidr.addr) || cidr.contains(o.addr)) {
                                        return Err(format!("% {} overlaps with {}", cidr.as_network(), other.name));
                                    }
                                }
                            }
                            d.interfaces[i].ip = Some(cidr);
                            d.interfaces[i].dhcp_client = false;
                        }
                        _ => d.interfaces[i].helper = if neg { None } else { Some(parse_ip(arg(t, 2)?)?) },
                    }
                }
                "encapsulation" => {
                    if !is_sub {
                        return Err("% Configura l'incapsulamento su una sottointerfaccia (es. interface g0/0.10)".into());
                    }
                    pick(arg(t, 1)?, &["dot1q"])?;
                    let v: u16 = arg(t, 2)?.parse().map_err(|_| INVALID)?;
                    if let IfaceKind::Sub { parent, .. } = d.interfaces[i].kind {
                        d.interfaces[i].kind = IfaceKind::Sub { parent, vlan: if neg { 0 } else { v } };
                    }
                }
                "switchport" => {
                    if !kind.is_l2() || !is_phys {
                        return Err(INVALID.into());
                    }
                    let sp = &mut d.interfaces[i].switchport;
                    match pick(arg(t, 1)?, &["mode", "access", "trunk", "nonegotiate", "port-security"])? {
                        "mode" => {
                            sp.mode = match pick(arg(t, 2)?, &["access", "trunk", "dynamic"])? {
                                "trunk" if !neg => PortMode::Trunk,
                                _ => PortMode::Access,
                            }
                        }
                        "access" => {
                            pick(arg(t, 2)?, &["vlan"])?;
                            if neg {
                                sp.access_vlan = 1;
                            } else {
                                let v: u16 = arg(t, 3)?.parse().map_err(|_| INVALID)?;
                                if !(1..=4094).contains(&v) {
                                    return Err(INVALID.into());
                                }
                                sp.access_vlan = v;
                                if !d.vlans.contains_key(&v) {
                                    d.vlans.insert(v, format!("VLAN{v:04}"));
                                    out = format!("% Access VLAN does not exist. Creating vlan {v}");
                                }
                            }
                        }
                        "trunk" => match pick(arg(t, 2)?, &["native", "allowed", "encapsulation"])? {
                            "native" => {
                                sp.native_vlan = if neg { 1 } else { arg(t, 4)?.parse().map_err(|_| INVALID)? };
                            }
                            "allowed" => {
                                if neg {
                                    sp.allowed = None;
                                } else {
                                    let spec = arg(t, 4)?;
                                    match spec.to_ascii_lowercase().as_str() {
                                        "all" => sp.allowed = None,
                                        "none" => sp.allowed = Some(vec![]),
                                        "add" | "remove" | "except" => {
                                            let list = parse_vlan_list(arg(t, 5)?).map_err(|e| format!("% {e}"))?;
                                            let mut cur = sp.allowed.clone().unwrap_or_else(|| (1..=4094).collect());
                                            match spec {
                                                "add" => cur.extend(list),
                                                "remove" => cur.retain(|v| !list.contains(v)),
                                                _ => cur = (1..=4094).filter(|v| !list.contains(v)).collect(),
                                            }
                                            cur.sort_unstable();
                                            cur.dedup();
                                            sp.allowed = if cur.len() == 4094 { None } else { Some(cur) };
                                        }
                                        _ => sp.allowed = Some(parse_vlan_list(spec).map_err(|e| format!("% {e}"))?),
                                    }
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                _ => {} // speed, duplex, ...: accettati
            }
        }
        for i in dhcp_ifaces {
            net.dhcp_request(dev, i);
            net.run_until_idle(REALTIME_MAX_TICKS);
            let ip = net.devices[&dev].interfaces[i].ip;
            out = match ip {
                Some(c) if !c.addr.is_link_local() => format!("%DHCP-6-ADDRESS_ASSIGN: assegnato {} con maschera {}", c.addr, c.mask()),
                _ => "% Nessuna risposta dal server DHCP".into(),
            };
        }
        Ok(out)
    }

    fn vlan_cmd(&mut self, net: &mut Network, dev: DeviceId, vlans: Vec<u16>, t: &[&str]) -> Res {
        pick(t[0], &["name"])?;
        let name = arg(t, 1)?.to_string();
        let d = net.devices.get_mut(&dev).unwrap();
        for v in vlans {
            d.vlans.insert(v, name.clone());
        }
        Ok(String::new())
    }

    fn dhcp_cmd(&mut self, net: &mut Network, dev: DeviceId, pool: String, t: &[&str]) -> Res {
        let cmd = pick(t[0], &["network", "default-router", "dns-server", "domain-name", "lease"])?;
        let d = net.devices.get_mut(&dev).unwrap();
        let p = d.dhcp_pools.iter_mut().find(|p| p.name == pool).ok_or("% pool inesistente")?;
        match cmd {
            "network" => {
                let ip = parse_ip(arg(t, 1)?)?;
                let prefix = parse_mask(arg(t, 2)?)?;
                p.network = Some(Ipv4Cidr::new(ip, prefix).as_network());
            }
            "default-router" => p.default_router = Some(parse_ip(arg(t, 1)?)?),
            "dns-server" => p.dns = Some(parse_ip(arg(t, 1)?)?),
            _ => {}
        }
        Ok(String::new())
    }

    fn line_cmd(&mut self, t: &[&str]) -> Res {
        pick(t[0], &["password", "login", "transport", "exec-timeout", "logging"])?;
        Ok(String::new())
    }

    // ------------------------------------------------------------------
    // Help contestuale
    // ------------------------------------------------------------------

    fn help(&self, net: &Network, dev: DeviceId, prefix: &str) -> String {
        let is_l2 = net.devices[&dev].kind.is_l2();
        let list: Vec<(&str, &str)> = match &self.mode {
            CliMode::User => vec![
                ("enable", "Entra in modalità privilegiata"),
                ("exit", "Chiude la sessione"),
                ("ping", "Invia messaggi ICMP echo"),
                ("show", "Mostra informazioni sul dispositivo"),
                ("traceroute", "Mostra il percorso verso una destinazione"),
            ],
            CliMode::Enable => vec![
                ("clear", "Azzera tabelle (mac address-table, arp-cache)"),
                ("configure", "Entra in modalità di configurazione (configure terminal)"),
                ("copy", "Salva la configurazione (copy running-config startup-config)"),
                ("disable", "Torna alla modalità utente"),
                ("ping", "Invia messaggi ICMP echo"),
                ("reload", "Riavvia il dispositivo"),
                ("show", "Mostra informazioni sul dispositivo"),
                ("traceroute", "Mostra il percorso verso una destinazione"),
                ("write", "Salva la configurazione"),
            ],
            CliMode::Config => {
                let mut v = vec![
                    ("banner", "Messaggio di benvenuto (banner motd #testo#)"),
                    ("do", "Esegue un comando della modalità privilegiata"),
                    ("enable", "Password della modalità privilegiata (enable secret)"),
                    ("end", "Torna alla modalità privilegiata"),
                    ("exit", "Esce dalla modalità di configurazione"),
                    ("hostname", "Imposta il nome del dispositivo"),
                    ("interface", "Configura un'interfaccia (anche interface range)"),
                    ("ip", "Comandi IP (route, dhcp, default-gateway, name-server)"),
                    ("line", "Configura console e terminali virtuali"),
                    ("no", "Annulla un comando"),
                ];
                if is_l2 {
                    v.push(("vlan", "Crea una VLAN e entra nella sua configurazione"));
                }
                v
            }
            CliMode::Iface(_) => {
                let mut v = vec![
                    ("description", "Descrizione dell'interfaccia"),
                    ("exit", "Torna alla configurazione globale"),
                    ("ip", "ip address <ip> <mask> | ip address dhcp | ip helper-address <ip>"),
                    ("no", "Annulla un comando"),
                    ("shutdown", "Spegne l'interfaccia (no shutdown la accende)"),
                ];
                if is_l2 {
                    v.push(("switchport", "mode access|trunk, access vlan N, trunk native|allowed vlan"));
                } else {
                    v.push(("encapsulation", "encapsulation dot1Q <vlan> (sottointerfacce)"));
                }
                v
            }
            CliMode::Vlan(_) => vec![("name", "Nome della VLAN"), ("exit", "Torna alla configurazione globale")],
            CliMode::DhcpPool(_) => vec![
                ("default-router", "Gateway assegnato ai client"),
                ("dns-server", "Server DNS assegnato ai client"),
                ("exit", "Torna alla configurazione globale"),
                ("network", "Rete del pool (network <ip> <mask>)"),
            ],
            CliMode::Line => vec![("password", "Password della linea"), ("login", "Richiede la password"), ("exit", "Esce")],
            CliMode::EnablePassword => vec![],
        };
        if prefix.contains(' ') {
            if prefix.to_ascii_lowercase().starts_with("sh") {
                return "  arp  cdp  interfaces  ip  mac-address-table  running-config  version  vlan  history\n  ip: interface brief | route | arp | dhcp binding | dhcp pool".into();
            }
            return "  <argomento>  vedi l'help del comando".into();
        }
        let p = prefix.to_ascii_lowercase();
        let mut out = String::new();
        for (c, h) in list.iter().filter(|(c, _)| c.starts_with(&p)) {
            let _ = writeln!(out, "  {c:<14} {h}");
        }
        if out.is_empty() {
            INVALID.to_string()
        } else {
            out
        }
    }
}

// ----------------------------------------------------------------------
// Output dei comandi show
// ----------------------------------------------------------------------

fn parse_range(net: &Network, dev: DeviceId, spec: &str) -> Result<Vec<usize>, String> {
    let d = &net.devices[&dev];
    let mut out = Vec::new();
    for part in spec.split(',') {
        let p: String = part.chars().filter(|c| !c.is_whitespace()).collect();
        if p.is_empty() {
            continue;
        }
        let (first, last) = p.split_once('-').unwrap_or((&p, ""));
        let full = expand_iface_name(first).ok_or("% Invalid interface")?;
        let (base, start) = full.rsplit_once('/').ok_or("% Invalid interface range")?;
        let start: u32 = start.parse().map_err(|_| "% Invalid interface range")?;
        let end: u32 = if last.is_empty() { start } else { last.parse().map_err(|_| "% Invalid interface range")? };
        for n in start..=end {
            let i = d.iface_by_name(&format!("{base}/{n}")).ok_or("% Invalid interface range")?;
            out.push(i);
        }
    }
    if out.is_empty() {
        return Err(INCOMPLETE.into());
    }
    Ok(out)
}

fn show_ip_int_brief(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let mut s = format!("{:<23}{:<16}{:<4}{:<7}{:<22}{}\n", "Interface", "IP-Address", "OK?", "Method", "Status", "Protocol");
    for (i, ifc) in d.interfaces.iter().enumerate() {
        let (st, proto) = net.port_status(Endpoint { device: dev, iface: i });
        let ip = ifc.ip.map(|c| c.addr.to_string()).unwrap_or_else(|| "unassigned".into());
        let method = if ifc.dhcp_client { "DHCP" } else if ifc.ip.is_some() { "manual" } else { "unset" };
        let _ = writeln!(s, "{:<23}{:<16}{:<4}{:<7}{:<22}{}", ifc.name, ip, "YES", method, st, proto);
    }
    s
}

fn show_ip_route(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let table = d.routing_table();
    let mut s = String::from("Codes: C - connected, L - local, S - static, * - candidate default\n\n");
    match table.iter().find(|r| r.network.prefix == 0) {
        Some(r) => {
            let _ = writeln!(s, "Gateway of last resort is {} to network 0.0.0.0\n", r.next_hop.unwrap());
        }
        None => s.push_str("Gateway of last resort is not set\n\n"),
    }
    let mut rows: Vec<(u32, u8, String)> = Vec::new();
    for r in &table {
        let ifn = &d.interfaces[r.iface].name;
        match r.kind {
            RouteKind::Connected => {
                rows.push((u32::from(r.network.addr), r.network.prefix, format!("C    {} is directly connected, {ifn}", r.network)));
                if let Some(ip) = d.interfaces[r.iface].ip {
                    rows.push((u32::from(ip.addr), 32, format!("L    {}/32 is directly connected, {ifn}", ip.addr)));
                }
            }
            RouteKind::Static | RouteKind::Default => {
                let code = if r.network.prefix == 0 { "S*" } else { "S " };
                rows.push((
                    u32::from(r.network.addr),
                    r.network.prefix,
                    format!("{code}   {} [1/0] via {}", r.network, r.next_hop.unwrap()),
                ));
            }
            RouteKind::Local => {}
        }
    }
    rows.sort();
    for (_, _, r) in rows {
        s.push_str(&r);
        s.push('\n');
    }
    s
}

fn show_arp(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let mut s = format!("{:<10}{:<17}{:<11}{:<16}{:<7}{}\n", "Protocol", "Address", "Age (min)", "Hardware Addr", "Type", "Interface");
    let iface_for = |ip: Ipv4Addr| {
        d.interfaces.iter().find(|i| i.ip.is_some_and(|c| c.contains(ip))).map(|i| i.name.clone()).unwrap_or_default()
    };
    let mut rows: Vec<(Ipv4Addr, String)> = Vec::new();
    for ifc in &d.interfaces {
        if let (Some(ip), true) = (ifc.ip, ifc.enabled) {
            rows.push((ip.addr, format!("{:<10}{:<17}{:<11}{:<16}{:<7}{}", "Internet", ip.addr, "-", ifc.mac, "ARPA", ifc.name)));
        }
    }
    for (ip, mac) in &d.state.arp_table {
        rows.push((*ip, format!("{:<10}{:<17}{:<11}{:<16}{:<7}{}", "Internet", ip, "0", mac, "ARPA", iface_for(*ip))));
    }
    rows.sort_by_key(|r| u32::from(r.0));
    for (_, r) in rows {
        s.push_str(&r);
        s.push('\n');
    }
    s
}

fn show_mac(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let mut s = String::from("          Mac Address Table\n-------------------------------------------\n\n");
    s.push_str("Vlan    Mac Address       Type        Ports\n----    -----------       --------    -----\n");
    for ((vlan, mac), port) in &d.state.mac_table {
        let _ = writeln!(s, "{:<8}{:<18}{:<12}{}", vlan, mac.to_string(), "DYNAMIC", short_iface_name(&d.interfaces[*port].name));
    }
    s
}

fn show_vlan(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let mut s = format!("{:<5}{:<33}{:<10}{}\n", "VLAN", "Name", "Status", "Ports");
    s.push_str("---- -------------------------------- --------- -------------------------------\n");
    for (id, name) in &d.vlans {
        let ports: Vec<String> = d
            .interfaces
            .iter()
            .filter(|i| i.is_physical() && i.switchport.mode == PortMode::Access && i.switchport.access_vlan == *id)
            .map(|i| short_iface_name(&i.name))
            .collect();
        let chunks: Vec<String> = ports.chunks(4).map(|c| c.join(", ")).collect();
        let first = chunks.first().cloned().unwrap_or_default();
        let _ = writeln!(s, "{:<5}{:<33}{:<10}{}", id, name, "active", first);
        for c in chunks.iter().skip(1) {
            let _ = writeln!(s, "{:<48}{}", "", c);
        }
    }
    s
}

fn show_trunk(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let trunks: Vec<(usize, &crate::device::Interface)> = d
        .interfaces
        .iter()
        .enumerate()
        .filter(|(_, i)| i.is_physical() && i.switchport.mode == PortMode::Trunk)
        .collect();
    let mut s = format!("{:<12}{:<13}{:<15}{:<14}{}\n", "Port", "Mode", "Encapsulation", "Status", "Native vlan");
    for (i, ifc) in &trunks {
        let up = net.port_status(Endpoint { device: dev, iface: *i }).1 == "up";
        let _ = writeln!(
            s,
            "{:<12}{:<13}{:<15}{:<14}{}",
            short_iface_name(&ifc.name),
            "on",
            "802.1q",
            if up { "trunking" } else { "not-trunking" },
            ifc.switchport.native_vlan
        );
    }
    let _ = writeln!(s, "\n{:<12}{}", "Port", "Vlans allowed on trunk");
    for (_, ifc) in &trunks {
        let allowed = ifc.switchport.allowed.as_ref().map(|a| format_vlan_list(a)).unwrap_or_else(|| "1-4094".into());
        let _ = writeln!(s, "{:<12}{}", short_iface_name(&ifc.name), allowed);
    }
    s
}

fn show_interface(net: &Network, dev: DeviceId, i: usize) -> String {
    let d = &net.devices[&dev];
    let ifc = &d.interfaces[i];
    let (st, proto) = net.port_status(Endpoint { device: dev, iface: i });
    let mut s = format!("{} is {st}, line protocol is {proto}\n", ifc.name);
    let _ = writeln!(s, "  Hardware is NPT virtual, address is {} (bia {})", ifc.mac, ifc.mac);
    if !ifc.description.is_empty() {
        let _ = writeln!(s, "  Description: {}", ifc.description);
    }
    if let Some(ip) = ifc.ip {
        let _ = writeln!(s, "  Internet address is {ip}");
    }
    if let IfaceKind::Sub { vlan, .. } = ifc.kind {
        let _ = writeln!(s, "  Encapsulation 802.1Q Virtual LAN, Vlan ID {vlan}");
    }
    s
}

fn show_dhcp_binding(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let mut s = format!("{:<17}{:<29}{:<19}{}\n", "IP address", "Client-ID/Hardware address", "Lease expiration", "Type");
    for l in &d.dhcp_leases {
        let _ = writeln!(s, "{:<17}{:<29}{:<19}{}", l.ip, l.mac.to_string(), "--", "Automatic");
    }
    s
}

fn show_dhcp_pool(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let mut s = String::new();
    for p in &d.dhcp_pools {
        let used = d.dhcp_leases.iter().filter(|l| p.network.is_some_and(|n| n.contains(l.ip))).count();
        let _ = writeln!(
            s,
            "Pool {} :\n Network: {}\n Default router: {}\n DNS server: {}\n Leased addresses: {used}\n",
            p.name,
            p.network.map(|n| n.to_string()).unwrap_or("-".into()),
            p.default_router.map(|n| n.to_string()).unwrap_or("-".into()),
            p.dns.map(|n| n.to_string()).unwrap_or("-".into()),
        );
    }
    s
}

fn show_neighbors(net: &Network, dev: DeviceId) -> String {
    let d = &net.devices[&dev];
    let mut s = format!("{:<17}{:<17}{:<12}{}\n", "Device ID", "Local Intrfce", "Platform", "Port ID");
    for (i, ifc) in d.interfaces.iter().enumerate() {
        if let Some((pd, pi)) = net.peer_of(Endpoint { device: dev, iface: i }) {
            let p = &net.devices[&pd];
            if p.kind.has_cli() {
                let _ = writeln!(s, "{:<17}{:<17}{:<12}{}", p.name, short_iface_name(&ifc.name), p.kind.model(), short_iface_name(&p.interfaces[pi].name));
            }
        }
    }
    s
}
