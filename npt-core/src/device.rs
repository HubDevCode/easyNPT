//! Dispositivi di rete e loro interfacce.

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
        DeviceKind::Pc,
        DeviceKind::Laptop,
        DeviceKind::Server,
        DeviceKind::Hub,
        DeviceKind::Switch,
        DeviceKind::Router,
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

    /// Prefisso usato per il nome automatico (PC0, Switch1, ...).
    pub fn name_prefix(&self) -> &'static str {
        self.label()
    }

    /// Il dispositivo è un terminale (ha un default gateway, non inoltra IP).
    pub fn is_host(&self) -> bool {
        matches!(self, DeviceKind::Pc | DeviceKind::Laptop | DeviceKind::Server)
    }

    /// Il dispositivo lavora solo a livello 1/2.
    pub fn is_l2(&self) -> bool {
        matches!(self, DeviceKind::Hub | DeviceKind::Switch)
    }

    pub fn is_router(&self) -> bool {
        matches!(self, DeviceKind::Router)
    }

    /// Nomi delle interfacce fisiche di default.
    pub fn default_ports(&self) -> Vec<String> {
        match self {
            DeviceKind::Pc | DeviceKind::Laptop | DeviceKind::Server => {
                vec!["FastEthernet0".into()]
            }
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
    for (long, short) in [
        ("GigabitEthernet", "Gi"),
        ("FastEthernet", "Fa"),
        ("Ethernet", "Eth"),
        ("Port", "P"),
    ] {
        if let Some(rest) = name.strip_prefix(long) {
            return format!("{short}{rest}");
        }
    }
    name.to_string()
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StaticRoute {
    pub network: Ipv4Cidr,
    pub next_hop: Ipv4Addr,
}

/// Una voce della tabella di routing calcolata.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RouteKind {
    Connected,
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

/// Stato di esecuzione: non viene salvato su file.
#[derive(Clone, Debug, Default)]
pub struct RuntimeState {
    pub arp_table: BTreeMap<Ipv4Addr, Mac>,
    /// Tabella MAC dello switch: MAC → indice della porta.
    pub mac_table: BTreeMap<Mac, usize>,
    pub pending: Vec<PendingPacket>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub kind: DeviceKind,
    pub name: String,
    pub pos: [f32; 2],
    pub interfaces: Vec<Interface>,
    /// Default gateway (solo per i terminali).
    #[serde(default)]
    pub gateway: Option<Ipv4Addr>,
    /// Rotte statiche (solo per i router).
    #[serde(default)]
    pub static_routes: Vec<StaticRoute>,
    #[serde(skip)]
    pub state: RuntimeState,
}

impl Device {
    pub fn new(id: DeviceId, kind: DeviceKind, name: String, pos: [f32; 2], macs: &mut impl FnMut() -> Mac) -> Self {
        let interfaces = kind
            .default_ports()
            .into_iter()
            .map(|name| Interface {
                name,
                mac: macs(),
                ip: None,
                // Come sui router reali, le interfacce dei router partono spente.
                enabled: !kind.is_router(),
                link: None,
            })
            .collect();
        Self {
            id,
            kind,
            name,
            pos,
            interfaces,
            gateway: None,
            static_routes: Vec::new(),
            state: RuntimeState::default(),
        }
    }

    pub fn iface_by_name(&self, name: &str) -> Option<usize> {
        let n = name.to_ascii_lowercase();
        self.interfaces.iter().position(|i| {
            i.name.to_ascii_lowercase() == n || short_iface_name(&i.name).to_ascii_lowercase() == n
        })
    }

    pub fn first_free_iface(&self) -> Option<usize> {
        self.interfaces.iter().position(|i| i.link.is_none())
    }

    /// L'indirizzo IP appartiene a una delle interfacce del dispositivo?
    pub fn owns_ip(&self, ip: Ipv4Addr) -> bool {
        self.interfaces.iter().any(|i| i.enabled && i.ip.map(|c| c.addr) == Some(ip))
    }

    /// Indirizzo IP "principale" (prima interfaccia configurata).
    pub fn primary_ip(&self) -> Option<Ipv4Cidr> {
        self.interfaces.iter().find_map(|i| i.ip)
    }

    /// Tabella di routing calcolata: connesse + statiche + default gateway.
    pub fn routing_table(&self) -> Vec<RouteEntry> {
        let mut table = Vec::new();
        for (idx, iface) in self.interfaces.iter().enumerate() {
            if let (Some(ip), true, Some(_)) = (iface.ip, iface.enabled, iface.link) {
                table.push(RouteEntry {
                    kind: RouteKind::Connected,
                    network: ip.as_network(),
                    next_hop: None,
                    iface: idx,
                });
            }
        }
        let connected: Vec<RouteEntry> = table.clone();
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
        }
        if self.kind.is_host() {
            if let Some(gw) = self.gateway {
                if let Some(iface) = resolve(gw) {
                    table.push(RouteEntry {
                        kind: RouteKind::Default,
                        network: Ipv4Cidr::new(Ipv4Addr::UNSPECIFIED, 0),
                        next_hop: Some(gw),
                        iface,
                    });
                }
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
}

/// Utilità per raggruppare porte libere per nome (usata dalla GUI).
pub fn free_ports(dev: &Device) -> Vec<(usize, String)> {
    dev.interfaces
        .iter()
        .enumerate()
        .filter(|(_, i)| i.link.is_none())
        .map(|(n, i)| (n, i.name.clone()))
        .collect()
}

