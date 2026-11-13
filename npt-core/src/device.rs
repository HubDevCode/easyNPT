//! Dispositivi di rete, interfacce e configurazione.

use crate::addr::{Ipv4Cidr, Mac};
use crate::packet::Ipv4Packet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

pub type DeviceId = u32;
pub type LinkId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum DeviceKind {
    Pc,
    Laptop,
    Server,
    Hub,
    Switch,
    Router,
}

impl DeviceKind {
    pub const ALL: [DeviceKind; 6] = [
        DeviceKind::Router,
        DeviceKind::Switch,
        DeviceKind::Hub,
        DeviceKind::Pc,
        DeviceKind::Laptop,
        DeviceKind::Server,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            DeviceKind::Pc => "PC",
            DeviceKind::Laptop => "Laptop",
            DeviceKind::Server => "Server",
            DeviceKind::Hub => "Hub",
            DeviceKind::Switch => "Switch",
            DeviceKind::Router => "Router",
        }
    }

    /// Modello mostrato nell'interfaccia.
    pub fn model(&self) -> &'static str {
        match self {
            DeviceKind::Pc => "NPT-PC",
            DeviceKind::Laptop => "NPT-Laptop",
            DeviceKind::Server => "NPT-Server",
            DeviceKind::Hub => "NPT-Hub8",
            DeviceKind::Switch => "NPT-SW24",
            DeviceKind::Router => "NPT-R3G",
        }
    }

    pub fn name_prefix(&self) -> &'static str {
        self.label()
    }

    /// Il dispositivo è un terminale (ha un default gateway, non inoltra IP).
    pub fn is_host(&self) -> bool {
        matches!(self, DeviceKind::Pc | DeviceKind::Laptop | DeviceKind::Server)
    }

    /// Il dispositivo inoltra frame a livello 1/2.
    pub fn is_l2(&self) -> bool {
        matches!(self, DeviceKind::Hub | DeviceKind::Switch)
    }

    pub fn is_router(&self) -> bool {
        matches!(self, DeviceKind::Router)
    }

    /// Ha una riga di comando in stile IOS.
    pub fn has_cli(&self) -> bool {
        matches!(self, DeviceKind::Router | DeviceKind::Switch)
    }

    /// Può avere un indirizzo IP (anche solo di gestione).
    pub fn has_ip(&self) -> bool {
        !matches!(self, DeviceKind::Hub)
    }

    pub fn default_ports(&self) -> Vec<String> {
        match self {
            DeviceKind::Pc | DeviceKind::Laptop | DeviceKind::Server => vec!["FastEthernet0".into()],
            DeviceKind::Hub => (0..8).map(|i| format!("Port{i}")).collect(),
            DeviceKind::Switch => {
                let mut v: Vec<String> = (1..=24).map(|i| format!("FastEthernet0/{i}")).collect();
                v.push("GigabitEthernet0/1".into());
                v.push("GigabitEthernet0/2".into());
                v
            }
            DeviceKind::Router => (0..3).map(|i| format!("GigabitEthernet0/{i}")).collect(),
        }
    }
}

/// Abbrevia un nome di interfaccia: `FastEthernet0/1` → `Fa0/1`.
pub fn short_iface_name(name: &str) -> String {
    for (long, short) in [("GigabitEthernet", "Gi"), ("FastEthernet", "Fa"), ("Ethernet", "Eth"), ("Port", "P")] {
        if let Some(rest) = name.strip_prefix(long) {
            return format!("{short}{rest}");
        }
    }
    name.to_string()
}

/// Espande un nome abbreviato (`g0/0`, `fa0/1`, `gig 0/0.10`, `vlan 1`)
/// nel nome completo (`GigabitEthernet0/0`, ...). Non verifica che esista.
pub fn expand_iface_name(input: &str) -> Option<String> {
    let s: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (ty, rest) = s.split_at(split);
    let ty = ty.to_ascii_lowercase();
    if ty.is_empty() {
        return None;
    }
    let full = ["GigabitEthernet", "FastEthernet", "Ethernet", "Vlan", "Port"]
        .into_iter()
        .find(|t| t.to_ascii_lowercase().starts_with(&ty))?;
    Some(format!("{full}{rest}"))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum PortMode {
    #[default]
    Access,
    Trunk,
}

/// Configurazione di livello 2 di una porta di switch.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SwitchPort {
    pub mode: PortMode,
    pub access_vlan: u16,
    pub native_vlan: u16,
    /// VLAN permesse sul trunk; `None` = tutte.
    pub allowed: Option<Vec<u16>>,
}

impl Default for SwitchPort {
    fn default() -> Self {
        Self { mode: PortMode::Access, access_vlan: 1, native_vlan: 1, allowed: None }
    }
}

impl SwitchPort {
    pub fn carries(&self, vlan: u16) -> bool {
        match self.mode {
            PortMode::Access => self.access_vlan == vlan,
            PortMode::Trunk => self.allowed.as_ref().is_none_or(|a| a.contains(&vlan)),
        }
    }

    /// VLAN di appartenenza di un frame in ingresso, `None` = da scartare.
    pub fn ingress_vlan(&self, tag: Option<u16>) -> Option<u16> {
        let v = match (self.mode, tag) {
            (PortMode::Access, None) => self.access_vlan,
            (PortMode::Access, Some(_)) => return None,
            (PortMode::Trunk, None) => self.native_vlan,
            (PortMode::Trunk, Some(t)) => t,
        };
        self.carries(v).then_some(v)
    }

    /// Tag da applicare in uscita: `None` = la porta non trasporta la VLAN,
    /// `Some(None)` = senza tag, `Some(Some(v))` = con tag 802.1Q.
    pub fn egress_tag(&self, vlan: u16) -> Option<Option<u16>> {
        if !self.carries(vlan) {
            return None;
        }
        match self.mode {
            PortMode::Access => Some(None),
            PortMode::Trunk if vlan == self.native_vlan => Some(None),
            PortMode::Trunk => Some(Some(vlan)),
        }
    }
}

/// Formatta un elenco di VLAN compattando gli intervalli: `1-5,10,20-22`.
pub fn format_vlan_list(v: &[u16]) -> String {
    let mut v = v.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        out.push(if start == end { start.to_string() } else { format!("{start}-{end}") });
        i += 1;
    }
    out.join(",")
}

/// Interpreta `1-5,10,20-22` in un elenco di VLAN.
pub fn parse_vlan_list(s: &str) -> Result<Vec<u16>, String> {
    let mut out = Vec::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let a: u16 = a.trim().parse().map_err(|_| format!("VLAN non valida: {part}"))?;
        let b: u16 = b.trim().parse().map_err(|_| format!("VLAN non valida: {part}"))?;
        if a == 0 || b > 4094 || a > b {
            return Err(format!("intervallo VLAN non valido: {part}"));
        }
        out.extend(a..=b);
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum IfaceKind {
    #[default]
    Physical,
    /// Sottointerfaccia di router con incapsulamento 802.1Q.
    Sub { parent: usize, vlan: u16 },
    /// Interfaccia virtuale di switch (SVI), es. `Vlan1`.
    Svi { vlan: u16 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interface {
    pub name: String,
    pub mac: Mac,
    pub ip: Option<Ipv4Cidr>,
    /// Stato amministrativo (equivalente a `shutdown` / `no shutdown`).
    pub enabled: bool,
    #[serde(default)]
    pub link: Option<LinkId>,
    #[serde(default)]
    pub kind: IfaceKind,
    #[serde(default)]
    pub switchport: SwitchPort,
    #[serde(default)]
    pub description: String,
    /// L'indirizzo viene ottenuto via DHCP.
    #[serde(default)]
    pub dhcp_client: bool,
    /// `ip helper-address`: inoltra le richieste DHCP a questo server.
    #[serde(default)]
    pub helper: Option<Ipv4Addr>,
}

impl Interface {
    pub fn is_physical(&self) -> bool {
        self.kind == IfaceKind::Physical
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StaticRoute {
    pub network: Ipv4Cidr,
    pub next_hop: Ipv4Addr,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DhcpPool {
    pub name: String,
    pub network: Option<Ipv4Cidr>,
    pub default_router: Option<Ipv4Addr>,
    pub dns: Option<Ipv4Addr>,
    /// Primo indirizzo assegnabile (se assente: primo host della rete).
    #[serde(default)]
    pub start: Option<Ipv4Addr>,
    /// Numero massimo di indirizzi (se assente: tutta la rete).
    #[serde(default)]
    pub max_users: Option<u32>,
}

impl DhcpPool {
    pub fn new(name: &str) -> Self {
        Self { name: name.into(), network: None, default_router: None, dns: None, start: None, max_users: None }
    }

    /// Indirizzi candidati in ordine.
    pub fn candidates(&self) -> Vec<Ipv4Addr> {
        let Some(net) = self.network else { return vec![] };
        let first = u32::from(net.network()) + 1;
        let last = u32::from(net.broadcast()).saturating_sub(1);
        let start = self.start.map(u32::from).unwrap_or(first).max(first);
        let max = self.max_users.unwrap_or(u32::MAX) as u64;
        (start..=last).take(max.min(65536) as usize).map(Ipv4Addr::from).collect()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Services {
    pub dhcp: bool,
    pub dns: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DhcpLease {
    pub mac: Mac,
    pub ip: Ipv4Addr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RouteKind {
    Connected,
    Local,
    Static,
    Default,
}

#[derive(Clone, Copy, Debug)]
pub struct RouteEntry {
    pub kind: RouteKind,
    pub network: Ipv4Cidr,
    /// `None` per le reti direttamente connesse.
    pub next_hop: Option<Ipv4Addr>,
    pub iface: usize,
}

/// Pacchetto in attesa che l'ARP risolva il next hop.
#[derive(Clone, Debug)]
pub struct PendingPacket {
    pub iface: usize,
    pub next_hop: Ipv4Addr,
    pub packet: Ipv4Packet,
    pub pdu_id: u64,
    pub since_tick: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DhcpClientState {
    Idle,
    Discovering { xid: u32, iface: usize },
    Requesting { xid: u32, iface: usize },
    Bound,
    Failed,
}

/// Stato di esecuzione: non viene salvato su file.
#[derive(Clone, Debug)]
pub struct RuntimeState {
    pub arp_table: BTreeMap<Ipv4Addr, Mac>,
    /// Tabella MAC dello switch: (VLAN, MAC) → indice della porta.
    pub mac_table: BTreeMap<(u16, Mac), usize>,
    pub pending: Vec<PendingPacket>,
    pub dhcp: DhcpClientState,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self { arp_table: BTreeMap::new(), mac_table: BTreeMap::new(), pending: Vec::new(), dhcp: DhcpClientState::Idle }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub kind: DeviceKind,
    pub name: String,
    pub pos: [f32; 2],
    pub interfaces: Vec<Interface>,
    /// Default gateway (terminali) o `ip default-gateway` (switch).
    #[serde(default)]
    pub gateway: Option<Ipv4Addr>,
    #[serde(default)]
    pub dns_server: Option<Ipv4Addr>,
    /// Rotte statiche (solo per i router).
    #[serde(default)]
    pub static_routes: Vec<StaticRoute>,
    /// Database VLAN (solo switch).
    #[serde(default)]
    pub vlans: BTreeMap<u16, String>,
    #[serde(default)]
    pub enable_secret: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub dhcp_pools: Vec<DhcpPool>,
    #[serde(default)]
    pub dhcp_excluded: Vec<(Ipv4Addr, Ipv4Addr)>,
    #[serde(default)]
    pub dhcp_leases: Vec<DhcpLease>,
    #[serde(default)]
    pub services: Services,
    #[serde(default)]
    pub dns_records: Vec<(String, Ipv4Addr)>,
    #[serde(skip)]
    pub state: RuntimeState,
}

impl Device {
    pub fn new(id: DeviceId, kind: DeviceKind, name: String, pos: [f32; 2], macs: &mut impl FnMut() -> Mac) -> Self {
        let mut interfaces: Vec<Interface> = kind
            .default_ports()
            .into_iter()
            .map(|name| Interface {
                name,
                mac: macs(),
                ip: None,
                // Come sui router reali, le interfacce dei router partono spente.
                enabled: !kind.is_router(),
                link: None,
                kind: IfaceKind::Physical,
                switchport: SwitchPort::default(),
                description: String::new(),
                dhcp_client: false,
                helper: None,
            })
            .collect();
        let mut vlans = BTreeMap::new();
        if kind == DeviceKind::Switch {
            vlans.insert(1, "default".to_string());
            interfaces.push(Interface {
                name: "Vlan1".into(),
                mac: macs(),
                ip: None,
                enabled: false,
                link: None,
                kind: IfaceKind::Svi { vlan: 1 },
                switchport: SwitchPort::default(),
                description: String::new(),
                dhcp_client: false,
                helper: None,
            });
        }
        Self {
            id,
            kind,
            name,
            pos,
            interfaces,
            gateway: None,
            dns_server: None,
            static_routes: Vec::new(),
            vlans,
            enable_secret: None,
            banner: None,
            dhcp_pools: Vec::new(),
            dhcp_excluded: Vec::new(),
            dhcp_leases: Vec::new(),
            services: Services::default(),
            dns_records: Vec::new(),
            state: RuntimeState::default(),
        }
    }

    /// Cerca un'interfaccia per nome completo o abbreviato.
    pub fn iface_by_name(&self, name: &str) -> Option<usize> {
        let full = expand_iface_name(name).unwrap_or_else(|| name.to_string()).to_ascii_lowercase();
        self.interfaces.iter().position(|i| i.name.to_ascii_lowercase() == full)
    }

    pub fn first_free_iface(&self) -> Option<usize> {
        self.interfaces.iter().position(|i| i.is_physical() && i.link.is_none())
    }

    /// Porta fisica che porta il traffico dell'interfaccia (logica o fisica).
    pub fn phys(&self, iface: usize) -> usize {
        match self.interfaces[iface].kind {
            IfaceKind::Sub { parent, .. } => parent,
            _ => iface,
        }
    }

    /// L'interfaccia è operativa (accesa e collegata)?
    pub fn iface_up(&self, iface: usize) -> bool {
        let i = &self.interfaces[iface];
        match i.kind {
            IfaceKind::Physical => i.enabled && i.link.is_some(),
            IfaceKind::Sub { parent, .. } => i.enabled && self.iface_up(parent),
            IfaceKind::Svi { vlan } => i.enabled && self.vlans.contains_key(&vlan),
        }
    }

    pub fn svi_for_vlan(&self, vlan: u16) -> Option<usize> {
        self.interfaces.iter().position(|i| i.kind == IfaceKind::Svi { vlan })
    }

    /// L'indirizzo IP appartiene a una delle interfacce operative del dispositivo?
    pub fn owns_ip(&self, ip: Ipv4Addr) -> bool {
        self.interfaces.iter().any(|i| i.enabled && i.ip.map(|c| c.addr) == Some(ip))
    }

    pub fn primary_ip(&self) -> Option<Ipv4Cidr> {
        self.interfaces.iter().find_map(|i| i.ip)
    }

    /// Crea (o restituisce) la sottointerfaccia `parent.n` di un router.
    pub fn ensure_subinterface(&mut self, parent: usize, number: u32) -> usize {
        let name = format!("{}.{number}", self.interfaces[parent].name);
        if let Some(i) = self.iface_by_name(&name) {
            return i;
        }
        let mac = self.interfaces[parent].mac;
        self.interfaces.push(Interface {
            name,
            mac,
            ip: None,
            enabled: true,
            link: None,
            kind: IfaceKind::Sub { parent, vlan: 0 },
            switchport: SwitchPort::default(),
            description: String::new(),
            dhcp_client: false,
            helper: None,
        });
        self.interfaces.len() - 1
    }

    /// Crea (o restituisce) l'interfaccia `VlanN` di uno switch.
    pub fn ensure_svi(&mut self, vlan: u16, mac: Mac) -> usize {
        if let Some(i) = self.svi_for_vlan(vlan) {
            return i;
        }
        self.interfaces.push(Interface {
            name: format!("Vlan{vlan}"),
            mac,
            ip: None,
            enabled: false,
            link: None,
            kind: IfaceKind::Svi { vlan },
            switchport: SwitchPort::default(),
            description: String::new(),
            dhcp_client: false,
            helper: None,
        });
        self.interfaces.len() - 1
    }

    /// Tabella di routing calcolata: connesse + statiche + default gateway.
    pub fn routing_table(&self) -> Vec<RouteEntry> {
        let mut table = Vec::new();
        for (idx, iface) in self.interfaces.iter().enumerate() {
            if let (Some(ip), true) = (iface.ip, self.iface_up(idx)) {
                table.push(RouteEntry { kind: RouteKind::Connected, network: ip.as_network(), next_hop: None, iface: idx });
            }
        }
        let connected = table.clone();
        let resolve = |nh: Ipv4Addr| connected.iter().find(|r| r.network.contains(nh)).map(|r| r.iface);

        if self.kind.is_router() {
            for r in &self.static_routes {
                if let Some(iface) = resolve(r.next_hop) {
                    table.push(RouteEntry {
                        kind: RouteKind::Static,
                        network: r.network.as_network(),
                        next_hop: Some(r.next_hop),
                        iface,
                    });
                }
            }
        } else if let Some(gw) = self.gateway {
            if let Some(iface) = resolve(gw) {
                table.push(RouteEntry {
                    kind: RouteKind::Default,
                    network: Ipv4Cidr::new(Ipv4Addr::UNSPECIFIED, 0),
                    next_hop: Some(gw),
                    iface,
                });
            }
        }
        table
    }

    /// Ricerca con "longest prefix match". Restituisce (interfaccia, next hop).
    pub fn lookup_route(&self, dst: Ipv4Addr) -> Option<(usize, Ipv4Addr)> {
        self.routing_table()
            .into_iter()
            .filter(|r| r.network.contains(dst))
            .max_by_key(|r| (r.network.prefix, r.kind == RouteKind::Connected))
            .map(|r| (r.iface, r.next_hop.unwrap_or(dst)))
    }

    pub fn is_excluded(&self, ip: Ipv4Addr) -> bool {
        self.dhcp_excluded.iter().any(|(a, b)| (u32::from(*a)..=u32::from(*b)).contains(&u32::from(ip)))
    }

    /// Il dispositivo fa da server DHCP?
    pub fn serves_dhcp(&self) -> bool {
        !self.dhcp_pools.is_empty() && (self.kind.is_router() || self.services.dhcp)
    }
}

/// Porte fisiche libere (usata dalla GUI per la scelta delle porte).
pub fn free_ports(dev: &Device) -> Vec<(usize, String)> {
    dev.interfaces
        .iter()
        .enumerate()
        .filter(|(_, i)| i.is_physical() && i.link.is_none())
        .map(|(n, i)| (n, i.name.clone()))
        .collect()
}
