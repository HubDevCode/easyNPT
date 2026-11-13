//! Motore di simulazione a eventi discreti.
//!
//! Il tempo avanza a "tick": a ogni tick tutti i frame in transito su un cavo
//! arrivano al dispositivo all'altra estremità, che li elabora e può generarne
//! di nuovi (in partenza al tick successivo).

use crate::addr::{Ipv4Cidr, Mac};
use crate::device::{
    short_iface_name, DeviceId, DeviceKind, DhcpClientState, DhcpLease, IfaceKind, LinkId, PendingPacket,
};
use crate::device::Device;
use crate::packet::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

/// Dopo quanti tick una richiesta ARP senza risposta viene abbandonata.
pub const ARP_TIMEOUT_TICKS: u64 = 12;
pub const DEFAULT_TTL_HOST: u8 = 128;
pub const DEFAULT_TTL_ROUTER: u8 = 255;
/// Limite di sicurezza per le esecuzioni "bloccanti" (CLI, tempo reale).
pub const REALTIME_MAX_TICKS: u64 = 5_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub device: DeviceId,
    pub iface: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum CableKind {
    CopperStraight,
    CopperCross,
}

impl CableKind {
    pub fn label(&self) -> &'static str {
        match self {
            CableKind::CopperStraight => "Rame dritto",
            CableKind::CopperCross => "Rame incrociato",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub id: LinkId,
    pub a: Endpoint,
    pub b: Endpoint,
    pub cable: CableKind,
}

impl Link {
    pub fn other(&self, from: Endpoint) -> Endpoint {
        if from == self.a { self.b } else { self.a }
    }
}

#[derive(Clone, Debug)]
pub struct InFlight {
    pub frame: Frame,
    pub link: LinkId,
    pub from: Endpoint,
    pub to: Endpoint,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventKind {
    Created,
    Sent,
    Received,
    Dropped,
    Info,
    Success,
    Failure,
}

impl EventKind {
    pub fn label(&self) -> &'static str {
        match self {
            EventKind::Created => "Creato",
            EventKind::Sent => "Inviato",
            EventKind::Received => "Ricevuto",
            EventKind::Dropped => "Scartato",
            EventKind::Info => "Info",
            EventKind::Success => "Successo",
            EventKind::Failure => "Fallito",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SimEvent {
    pub tick: u64,
    pub device: DeviceId,
    pub kind: EventKind,
    pub info: String,
    pub frame: Option<Frame>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PingStatus {
    Pending,
    Success { ticks: u64, ttl: u8 },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct PingRecord {
    pub src: DeviceId,
    pub dst: Ipv4Addr,
    pub id: u16,
    pub seq: u16,
    pub sent_tick: u64,
    pub status: PingStatus,
    /// Chi ha risposto (anche con un errore ICMP).
    pub reply_from: Option<Ipv4Addr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DnsStatus {
    Pending,
    Resolved(Ipv4Addr),
    NotFound,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct DnsLookup {
    pub device: DeviceId,
    pub id: u16,
    pub name: String,
    pub server: Ipv4Addr,
    pub status: DnsStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraceHop {
    pub ttl: u8,
    pub from: Option<Ipv4Addr>,
    pub reached: bool,
}

/// Nota di testo sull'area di lavoro.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Note {
    pub pos: [f32; 2],
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Network {
    pub devices: BTreeMap<DeviceId, Device>,
    pub links: BTreeMap<LinkId, Link>,
    #[serde(default)]
    pub notes: Vec<Note>,
    next_device_id: DeviceId,
    next_link_id: LinkId,
    next_mac: u32,

    #[serde(skip)]
    pub in_flight: Vec<InFlight>,
    #[serde(skip)]
    pub tick: u64,
    #[serde(skip)]
    pub events: Vec<SimEvent>,
    #[serde(skip)]
    pub pings: Vec<PingRecord>,
    #[serde(skip)]
    pub dns_lookups: Vec<DnsLookup>,
    #[serde(skip)]
    next_pdu: u64,
    #[serde(skip)]
    ping_seq: u16,
    #[serde(skip)]
    next_xid: u32,
}

impl Network {
    pub fn new() -> Self {
        Self::default()
    }

    // ------------------------------------------------------------------
    // Topologia
    // ------------------------------------------------------------------

    fn unique_name(&self, kind: DeviceKind) -> String {
        let prefix = kind.name_prefix();
        (0..)
            .map(|n| format!("{prefix}{n}"))
            .find(|n| !self.devices.values().any(|d| &d.name == n))
            .expect("contatore infinito")
    }

    pub fn alloc_mac(&mut self) -> Mac {
        self.next_mac += 1;
        Mac::from_counter(self.next_mac)
    }

    pub fn add_device(&mut self, kind: DeviceKind, pos: [f32; 2]) -> DeviceId {
        let id = self.next_device_id;
        self.next_device_id += 1;
        let name = self.unique_name(kind);
        let mut counter = self.next_mac;
        let dev = Device::new(id, kind, name, pos, &mut || {
            counter += 1;
            Mac::from_counter(counter)
        });
        self.next_mac = counter;
        self.devices.insert(id, dev);
        id
    }

    pub fn remove_device(&mut self, id: DeviceId) {
        let links: Vec<LinkId> =
            self.links.values().filter(|l| l.a.device == id || l.b.device == id).map(|l| l.id).collect();
        for l in links {
            self.disconnect(l);
        }
        self.devices.remove(&id);
        self.in_flight.retain(|f| f.to.device != id && f.from.device != id);
    }

    pub fn device(&self, id: DeviceId) -> Option<&Device> {
        self.devices.get(&id)
    }

    pub fn device_mut(&mut self, id: DeviceId) -> Option<&mut Device> {
        self.devices.get_mut(&id)
    }

    pub fn device_by_name(&self, name: &str) -> Option<DeviceId> {
        self.devices.values().find(|d| d.name.eq_ignore_ascii_case(name)).map(|d| d.id)
    }

    pub fn device_name(&self, id: DeviceId) -> String {
        self.devices.get(&id).map(|d| d.name.clone()).unwrap_or_else(|| format!("#{id}"))
    }

    /// Collega due interfacce fisiche.
    pub fn connect(&mut self, a: Endpoint, b: Endpoint) -> Result<LinkId, String> {
        if a.device == b.device {
            return Err("non si può collegare un dispositivo a se stesso".into());
        }
        for ep in [a, b] {
            let dev = self.devices.get(&ep.device).ok_or("dispositivo inesistente")?;
            let iface = dev.interfaces.get(ep.iface).ok_or("interfaccia inesistente")?;
            if !iface.is_physical() {
                return Err(format!("{} non è una porta fisica", iface.name));
            }
            if iface.link.is_some() {
                return Err(format!("{} {} è già collegata", dev.name, iface.name));
            }
        }
        let kind_a = self.devices[&a.device].kind;
        let kind_b = self.devices[&b.device].kind;
        let cable = if kind_a.is_l2() == kind_b.is_l2() { CableKind::CopperCross } else { CableKind::CopperStraight };
        let id = self.next_link_id;
        self.next_link_id += 1;
        self.links.insert(id, Link { id, a, b, cable });
        for ep in [a, b] {
            self.devices.get_mut(&ep.device).unwrap().interfaces[ep.iface].link = Some(id);
        }
        Ok(id)
    }

    /// Collega due dispositivi usando la prima porta libera di ciascuno.
    pub fn connect_auto(&mut self, a: DeviceId, b: DeviceId) -> Result<LinkId, String> {
        let da = self.devices.get(&a).ok_or("dispositivo inesistente")?;
        let db = self.devices.get(&b).ok_or("dispositivo inesistente")?;
        let ia = da.first_free_iface().ok_or_else(|| format!("{} non ha porte libere", da.name))?;
        let ib = db.first_free_iface().ok_or_else(|| format!("{} non ha porte libere", db.name))?;
        self.connect(Endpoint { device: a, iface: ia }, Endpoint { device: b, iface: ib })
    }

    pub fn disconnect(&mut self, link: LinkId) {
        if let Some(l) = self.links.remove(&link) {
            for ep in [l.a, l.b] {
                if let Some(d) = self.devices.get_mut(&ep.device) {
                    d.interfaces[ep.iface].link = None;
                    d.state.mac_table.retain(|_, port| *port != ep.iface);
                }
            }
            self.in_flight.retain(|f| f.link != link);
        }
    }

    /// Il collegamento è attivo (entrambe le interfacce accese)?
    pub fn link_is_up(&self, link: &Link) -> bool {
        [link.a, link.b].iter().all(|ep| {
            self.devices.get(&ep.device).and_then(|d| d.interfaces.get(ep.iface)).is_some_and(|i| i.enabled)
        })
    }

    /// Stato operativo di una porta fisica: (stato linea, protocollo).
    pub fn port_status(&self, ep: Endpoint) -> (&'static str, &'static str) {
        let dev = &self.devices[&ep.device];
        let ifc = &dev.interfaces[ep.iface];
        if !ifc.enabled {
            return ("administratively down", "down");
        }
        match ifc.kind {
            IfaceKind::Physical => match ifc.link.and_then(|l| self.links.get(&l)) {
                Some(l) if self.link_is_up(l) => ("up", "up"),
                _ => ("down", "down"),
            },
            IfaceKind::Sub { parent, .. } => self.port_status(Endpoint { device: ep.device, iface: parent }),
            IfaceKind::Svi { vlan } => {
                // Una SVI è "up" se almeno una porta attiva trasporta la sua VLAN.
                let any = dev.interfaces.iter().enumerate().any(|(i, p)| {
                    p.is_physical()
                        && p.switchport.carries(vlan)
                        && self.port_status(Endpoint { device: ep.device, iface: i }).1 == "up"
                });
                if any && dev.vlans.contains_key(&vlan) { ("up", "up") } else { ("up", "down") }
            }
        }
    }

    /// Estremità opposta di un'interfaccia fisica.
    pub fn peer_of(&self, ep: Endpoint) -> Option<(DeviceId, usize)> {
        let dev = self.devices.get(&ep.device)?;
        let link = self.links.get(&dev.interfaces.get(ep.iface)?.link?)?;
        let o = link.other(ep);
        Some((o.device, o.iface))
    }

    pub fn endpoint_label(&self, ep: Endpoint) -> String {
        match self.devices.get(&ep.device) {
            Some(d) => format!("{} {}", d.name, short_iface_name(&d.interfaces[ep.iface].name)),
            None => "?".into(),
        }
    }

    // ------------------------------------------------------------------
    // Stato di simulazione
    // ------------------------------------------------------------------

    /// Azzera tutto lo stato dinamico (tabelle ARP/MAC, pacchetti, eventi).
    pub fn reset_runtime(&mut self) {
        self.in_flight.clear();
        self.events.clear();
        self.pings.clear();
        self.dns_lookups.clear();
        self.tick = 0;
        for d in self.devices.values_mut() {
            d.state = Default::default();
        }
    }

    pub fn is_idle(&self) -> bool {
        self.in_flight.is_empty() && self.devices.values().all(|d| d.state.pending.is_empty())
    }

    /// Avanza la simulazione di un tick.
    pub fn step(&mut self) {
        self.tick += 1;
        let frames = std::mem::take(&mut self.in_flight);
        for f in frames {
            self.receive(f.to, f.frame);
        }
        self.expire_pending_arp();
        if self.is_idle() {
            self.finalize();
        }
    }

    /// Esegue tick finché la rete non è inattiva (o si raggiunge il limite).
    pub fn run_until_idle(&mut self, max_ticks: u64) {
        let start = self.tick;
        while !self.is_idle() && self.tick - start < max_ticks {
            self.step();
        }
        self.finalize();
    }

    /// Chiude le operazioni rimaste senza risposta a rete inattiva.
    fn finalize(&mut self) {
        if !self.in_flight.is_empty() {
            return;
        }
        let mut failed = Vec::new();
        for p in self.pings.iter_mut().filter(|p| p.status == PingStatus::Pending) {
            p.status = PingStatus::Failed("Request timed out".into());
            failed.push((p.src, p.dst));
        }
        for (src, dst) in failed {
            self.log(src, EventKind::Failure, format!("Ping verso {dst}: request timed out"), None);
        }
        for l in self.dns_lookups.iter_mut().filter(|l| l.status == DnsStatus::Pending) {
            l.status = DnsStatus::Failed("timeout del server DNS".into());
        }
        let stuck: Vec<DeviceId> = self
            .devices
            .values()
            .filter(|d| matches!(d.state.dhcp, DhcpClientState::Discovering { .. } | DhcpClientState::Requesting { .. }))
            .map(|d| d.id)
            .collect();
        for id in stuck {
            self.dhcp_fail(id);
        }
    }

    fn log(&mut self, device: DeviceId, kind: EventKind, info: String, frame: Option<&Frame>) {
        self.events.push(SimEvent { tick: self.tick, device, kind, info, frame: frame.cloned() });
    }

    fn new_pdu(&mut self) -> u64 {
        self.next_pdu += 1;
        self.next_pdu
    }

    fn new_xid(&mut self) -> u32 {
        self.next_xid = self.next_xid.wrapping_add(0x1f3d_5b79);
        self.next_xid
    }

    // ------------------------------------------------------------------
    // Comandi utente: ping, traceroute, DHCP, DNS
    // ------------------------------------------------------------------

    /// Invia un ICMP Echo Request da `src` verso `dst`.
    pub fn ping(&mut self, src: DeviceId, dst: Ipv4Addr) -> Result<(), String> {
        self.ping_ttl(src, dst, None).map(|_| ())
    }

    /// Come [`ping`], con TTL esplicito. Restituisce l'indice del record.
    pub fn ping_ttl(&mut self, src: DeviceId, dst: Ipv4Addr, ttl: Option<u8>) -> Result<usize, String> {
        let dev = self.devices.get(&src).ok_or("dispositivo inesistente")?;
        if !dev.kind.has_ip() {
            return Err(format!("{} non ha un indirizzo IP", dev.name));
        }
        self.ping_seq = self.ping_seq.wrapping_add(1);
        let seq = self.ping_seq;
        let id = src as u16;
        let ttl = ttl.unwrap_or(if dev.kind.is_host() { DEFAULT_TTL_HOST } else { DEFAULT_TTL_ROUTER });
        let owns = dev.owns_ip(dst);
        let route = dev.lookup_route(dst);
        let src_name = dev.name.clone();

        let mut rec =
            PingRecord { src, dst, id, seq, sent_tick: self.tick, status: PingStatus::Pending, reply_from: None };
        let idx = self.pings.len();
        if owns {
            rec.status = PingStatus::Success { ticks: 0, ttl };
            rec.reply_from = Some(dst);
            self.pings.push(rec);
            self.log(src, EventKind::Success, format!("Ping verso se stesso ({dst})"), None);
            return Ok(idx);
        }
        let Some((iface, next_hop)) = route else {
            rec.status = PingStatus::Failed("nessuna rotta verso la destinazione".into());
            self.pings.push(rec);
            self.log(src, EventKind::Failure, format!("{src_name}: nessuna rotta verso {dst}"), None);
            return Ok(idx);
        };
        let src_ip = self.devices[&src].interfaces[iface].ip.expect("rotta senza IP").addr;
        self.pings.push(rec);
        let packet = Ipv4Packet {
            src: src_ip,
            dst,
            ttl,
            payload: L4::Icmp(IcmpPacket { kind: IcmpKind::EchoRequest, id, seq }),
        };
        let pdu = self.new_pdu();
        self.log(src, EventKind::Created, format!("ICMP Echo Request verso {dst}"), None);
        self.transmit_ip(src, iface, next_hop, packet, pdu);
        Ok(idx)
    }

    /// Traceroute "bloccante": esegue un probe per TTL fino a destinazione.
    pub fn traceroute(&mut self, src: DeviceId, dst: Ipv4Addr, max_hops: u8) -> Result<Vec<TraceHop>, String> {
        let mut hops = Vec::new();
        let mut silent = 0;
        for ttl in 1..=max_hops {
            let idx = self.ping_ttl(src, dst, Some(ttl))?;
            self.run_until_idle(REALTIME_MAX_TICKS);
            let rec = self.pings[idx].clone();
            let reached = matches!(rec.status, PingStatus::Success { .. });
            hops.push(TraceHop { ttl, from: rec.reply_from, reached });
            if reached {
                break;
            }
            if let PingStatus::Failed(r) = &rec.status {
                if r.contains("unreachable") || r.contains("nessuna rotta") {
                    break;
                }
            }
            silent = if rec.reply_from.is_none() { silent + 1 } else { 0 };
            if silent >= 4 {
                break;
            }
        }
        Ok(hops)
    }

    /// Avvia la richiesta DHCP su un'interfaccia (DISCOVER in broadcast).
    pub fn dhcp_request(&mut self, dev_id: DeviceId, iface: usize) {
        let xid = self.new_xid();
        let dev = self.devices.get_mut(&dev_id).unwrap();
        let ifc = &mut dev.interfaces[iface];
        ifc.dhcp_client = true;
        ifc.ip = None;
        let mac = ifc.mac;
        dev.gateway = None;
        dev.state.dhcp = DhcpClientState::Discovering { xid, iface };
        let msg = DhcpMessage {
            op: DhcpOp::Discover,
            xid,
            client_mac: mac,
            your_ip: Ipv4Addr::UNSPECIFIED,
            server_ip: Ipv4Addr::UNSPECIFIED,
            prefix: 0,
            router: None,
            dns: None,
            relay: Ipv4Addr::UNSPECIFIED,
        };
        self.log(dev_id, EventKind::Created, "DHCP DISCOVER".into(), None);
        self.send_dhcp_broadcast(dev_id, iface, Ipv4Addr::UNSPECIFIED, PORT_DHCP_CLIENT, PORT_DHCP_SERVER, msg);
    }

    /// Torna a configurazione statica (senza indirizzo).
    pub fn dhcp_release(&mut self, dev_id: DeviceId, iface: usize) {
        let dev = self.devices.get_mut(&dev_id).unwrap();
        let mac = dev.interfaces[iface].mac;
        dev.interfaces[iface].ip = None;
        dev.state.dhcp = DhcpClientState::Idle;
        for d in self.devices.values_mut() {
            d.dhcp_leases.retain(|l| l.mac != mac);
        }
    }

    fn dhcp_fail(&mut self, dev_id: DeviceId) {
        let dev = self.devices.get_mut(&dev_id).unwrap();
        let iface = match dev.state.dhcp {
            DhcpClientState::Discovering { iface, .. } | DhcpClientState::Requesting { iface, .. } => iface,
            _ => return,
        };
        dev.state.dhcp = DhcpClientState::Failed;
        // Indirizzo APIPA (link-local) come fanno i sistemi operativi reali.
        let m = dev.interfaces[iface].mac.0;
        let apipa = Ipv4Addr::new(169, 254, m[4].max(1), m[5].max(1));
        dev.interfaces[iface].ip = Some(Ipv4Cidr::new(apipa, 16));
        self.log(dev_id, EventKind::Failure, format!("DHCP fallito: assegnato APIPA {apipa}"), None);
    }

    /// Avvia una risoluzione DNS. Restituisce l'indice della richiesta.
    pub fn dns_resolve(&mut self, dev_id: DeviceId, name: &str) -> usize {
        let dev = &self.devices[&dev_id];
        let idx = self.dns_lookups.len();
        let Some(server) = dev.dns_server else {
            self.dns_lookups.push(DnsLookup {
                device: dev_id,
                id: 0,
                name: name.into(),
                server: Ipv4Addr::UNSPECIFIED,
                status: DnsStatus::Failed("nessun server DNS configurato".into()),
            });
            return idx;
        };
        let id = (idx as u16).wrapping_add(1000);
        self.dns_lookups.push(DnsLookup { device: dev_id, id, name: name.into(), server, status: DnsStatus::Pending });
        let packet = Ipv4Packet {
            src: Ipv4Addr::UNSPECIFIED, // completato da send_ip
            dst: server,
            ttl: DEFAULT_TTL_HOST,
            payload: L4::Udp(UdpDatagram {
                src_port: 1025 + (id % 5000),
                dst_port: PORT_DNS,
                payload: AppData::Dns(DnsMessage { id, response: false, name: name.into(), answer: None }),
            }),
        };
        let pdu = self.new_pdu();
        self.log(dev_id, EventKind::Created, format!("DNS query {name}"), None);
        if !self.send_ip(dev_id, packet, pdu) {
            self.dns_lookups[idx].status = DnsStatus::Failed("nessuna rotta verso il server DNS".into());
        }
        idx
    }

    /// Risolve un nome (o accetta direttamente un IP) in modo bloccante.
    pub fn resolve_blocking(&mut self, dev_id: DeviceId, target: &str) -> Result<Ipv4Addr, String> {
        if let Ok(ip) = target.parse::<Ipv4Addr>() {
            return Ok(ip);
        }
        let idx = self.dns_resolve(dev_id, target);
        self.run_until_idle(REALTIME_MAX_TICKS);
        match &self.dns_lookups[idx].status {
            DnsStatus::Resolved(ip) => Ok(*ip),
            DnsStatus::NotFound => Err(format!("nome '{target}' non trovato")),
            DnsStatus::Failed(e) => Err(e.clone()),
            DnsStatus::Pending => Err("timeout".into()),
        }
    }

    // ------------------------------------------------------------------
    // Livello fisico / collegamento
    // ------------------------------------------------------------------

    /// Invia un frame da un'interfaccia (fisica, sottointerfaccia o SVI).
    fn emit(&mut self, dev_id: DeviceId, iface: usize, mut frame: Frame) {
        let ifc = &self.devices[&dev_id].interfaces[iface];
        let short = short_iface_name(&ifc.name);
        if !ifc.enabled {
            self.log(dev_id, EventKind::Dropped, format!("{short} è spenta (shutdown)"), Some(&frame));
            return;
        }
        match ifc.kind {
            IfaceKind::Physical => self.emit_phys(dev_id, iface, frame),
            IfaceKind::Sub { parent, vlan } => {
                if vlan == 0 {
                    self.log(dev_id, EventKind::Dropped, format!("{short}: manca 'encapsulation dot1Q'"), Some(&frame));
                    return;
                }
                frame.vlan = Some(vlan);
                self.emit_phys(dev_id, parent, frame);
            }
            IfaceKind::Svi { vlan } => self.switch_forward(dev_id, vlan, frame, None),
        }
    }

    fn emit_phys(&mut self, dev_id: DeviceId, iface: usize, frame: Frame) {
        let ifc = &self.devices[&dev_id].interfaces[iface];
        let short = short_iface_name(&ifc.name);
        if !ifc.enabled {
            self.log(dev_id, EventKind::Dropped, format!("{short} è spenta (shutdown)"), Some(&frame));
            return;
        }
        let Some(link_id) = ifc.link else {
            self.log(dev_id, EventKind::Dropped, format!("{short} non è collegata"), Some(&frame));
            return;
        };
        let from = Endpoint { device: dev_id, iface };
        let to = self.links[&link_id].other(from);
        let info = format!("{short} → {}", self.endpoint_label(to));
        self.log(dev_id, EventKind::Sent, info, Some(&frame));
        self.in_flight.push(InFlight { frame, link: link_id, from, to });
    }

    fn receive(&mut self, to: Endpoint, frame: Frame) {
        let Some(dev) = self.devices.get(&to.device) else { return };
        if !dev.interfaces[to.iface].enabled {
            let short = short_iface_name(&dev.interfaces[to.iface].name);
            self.log(to.device, EventKind::Dropped, format!("{short} è spenta"), Some(&frame));
            return;
        }
        match dev.kind {
            DeviceKind::Hub => self.hub_receive(to, frame),
            DeviceKind::Switch => self.switch_receive(to, frame),
            _ => self.l3_dispatch(to, frame),
        }
    }

    fn hub_receive(&mut self, to: Endpoint, frame: Frame) {
        self.log(to.device, EventKind::Received, "ripete su tutte le altre porte".into(), Some(&frame));
        let ports: Vec<usize> = self.devices[&to.device]
            .interfaces
            .iter()
            .enumerate()
            .filter(|(i, p)| *i != to.iface && p.link.is_some() && p.enabled)
            .map(|(i, _)| i)
            .collect();
        for p in ports {
            self.emit_phys(to.device, p, frame.clone());
        }
    }

    fn switch_receive(&mut self, to: Endpoint, mut frame: Frame) {
        let dev = self.devices.get_mut(&to.device).unwrap();
        let port = dev.interfaces[to.iface].switchport.clone();
        let Some(vlan) = port.ingress_vlan(frame.vlan) else {
            let why = match frame.vlan {
                Some(v) => format!("frame con tag VLAN {v} non ammesso sulla porta"),
                None => "VLAN nativa non ammessa".into(),
            };
            self.log(to.device, EventKind::Dropped, why, Some(&frame));
            return;
        };
        if !dev.vlans.contains_key(&vlan) {
            self.log(to.device, EventKind::Dropped, format!("la VLAN {vlan} non esiste sullo switch"), Some(&frame));
            return;
        }
        if !frame.src.is_broadcast() {
            dev.state.mac_table.insert((vlan, frame.src), to.iface);
        }
        frame.vlan = None;
        // Frame per l'interfaccia di gestione (SVI) dello switch.
        if let Some(svi) = dev.svi_for_vlan(vlan) {
            let s = &dev.interfaces[svi];
            if s.enabled && (frame.dst == s.mac || frame.dst.is_broadcast()) {
                let for_svi = frame.dst == s.mac;
                self.l3_receive(Endpoint { device: to.device, iface: svi }, frame.clone());
                if for_svi {
                    return;
                }
            }
        }
        self.switch_forward(to.device, vlan, frame, Some(to.iface));
    }

    /// Inoltro di uno switch all'interno di una VLAN.
    fn switch_forward(&mut self, dev_id: DeviceId, vlan: u16, frame: Frame, ingress: Option<usize>) {
        let dev = &self.devices[&dev_id];
        let known = if frame.dst.is_broadcast() { None } else { dev.state.mac_table.get(&(vlan, frame.dst)).copied() };
        let ports: Vec<usize> = match known {
            Some(p) if Some(p) == ingress => {
                self.log(dev_id, EventKind::Dropped, "destinazione sulla stessa porta: filtrato".into(), Some(&frame));
                return;
            }
            Some(p) => {
                let short = short_iface_name(&dev.interfaces[p].name);
                self.log(dev_id, EventKind::Received, format!("VLAN {vlan}: MAC noto su {short}"), Some(&frame));
                vec![p]
            }
            None => {
                let ports: Vec<usize> = dev
                    .interfaces
                    .iter()
                    .enumerate()
                    .filter(|(i, p)| {
                        Some(*i) != ingress && p.is_physical() && p.link.is_some() && p.enabled && p.switchport.carries(vlan)
                    })
                    .map(|(i, _)| i)
                    .collect();
                let why = if frame.dst.is_broadcast() { "broadcast" } else { "MAC sconosciuto" };
                self.log(dev_id, EventKind::Received, format!("VLAN {vlan}: {why}, flooding"), Some(&frame));
                ports
            }
        };
        for p in ports {
            let Some(tag) = self.devices[&dev_id].interfaces[p].switchport.egress_tag(vlan) else { continue };
            let mut f = frame.clone();
            f.vlan = tag;
            self.emit_phys(dev_id, p, f);
        }
    }

    /// Smista un frame ricevuto da router/host sull'interfaccia logica giusta.
    fn l3_dispatch(&mut self, to: Endpoint, mut frame: Frame) {
        let dev = &self.devices[&to.device];
        let logical = match frame.vlan {
            None => to.iface,
            Some(v) => {
                let sub = dev.interfaces.iter().position(|i| i.kind == IfaceKind::Sub { parent: to.iface, vlan: v });
                match sub {
                    Some(s) if dev.interfaces[s].enabled => s,
                    _ => {
                        self.log(to.device, EventKind::Dropped, format!("nessuna sottointerfaccia per VLAN {v}"), Some(&frame));
                        return;
                    }
                }
            }
        };
        frame.vlan = None;
        self.l3_receive(Endpoint { device: to.device, iface: logical }, frame);
    }

    // ------------------------------------------------------------------
    // Livello 3 (terminali, router, SVI degli switch)
    // ------------------------------------------------------------------

    fn l3_receive(&mut self, to: Endpoint, frame: Frame) {
        let dev = &self.devices[&to.device];
        let ifc = &dev.interfaces[to.iface];
        if frame.dst != ifc.mac && !frame.dst.is_broadcast() {
            self.log(to.device, EventKind::Dropped, "MAC di destinazione non corrisponde".into(), Some(&frame));
            return;
        }
        match frame.payload.clone() {
            EtherPayload::Arp(arp) => self.arp_receive(to, arp, &frame),
            EtherPayload::Ipv4(p) => {
                let dev = &self.devices[&to.device];
                let is_bcast = p.dst == Ipv4Addr::BROADCAST
                    || dev.interfaces[to.iface].ip.is_some_and(|c| c.prefix < 31 && c.broadcast() == p.dst);
                if dev.owns_ip(p.dst) || is_bcast {
                    self.local_deliver(to, p, &frame);
                } else if dev.kind.is_router() {
                    self.forward(to, p, &frame);
                } else {
                    self.log(to.device, EventKind::Dropped, format!("IP {} non è mio", p.dst), Some(&frame));
                }
            }
        }
    }

    fn arp_receive(&mut self, to: Endpoint, arp: ArpPacket, frame: &Frame) {
        let dev = self.devices.get_mut(&to.device).unwrap();
        let my = dev.interfaces[to.iface].ip;
        let my_mac = dev.interfaces[to.iface].mac;
        let for_me = my.is_some_and(|c| c.addr == arp.target_ip);
        if for_me || dev.state.arp_table.contains_key(&arp.sender_ip) {
            dev.state.arp_table.insert(arp.sender_ip, arp.sender_mac);
        }
        if !for_me {
            self.log(to.device, EventKind::Dropped, format!("ARP per {}: non sono io", arp.target_ip), Some(frame));
            return;
        }
        match arp.op {
            ArpOp::Request => {
                self.log(to.device, EventKind::Received, format!("ARP request da {}: rispondo", arp.sender_ip), Some(frame));
                let reply = Frame {
                    pdu_id: frame.pdu_id,
                    src: my_mac,
                    dst: arp.sender_mac,
                    vlan: None,
                    payload: EtherPayload::Arp(ArpPacket {
                        op: ArpOp::Reply,
                        sender_mac: my_mac,
                        sender_ip: arp.target_ip,
                        target_mac: arp.sender_mac,
                        target_ip: arp.sender_ip,
                    }),
                };
                self.emit(to.device, to.iface, reply);
            }
            ArpOp::Reply => {
                self.log(to.device, EventKind::Received, format!("ARP: {} è {}", arp.sender_ip, arp.sender_mac), Some(frame));
            }
        }
        self.flush_pending(to.device, arp.sender_ip);
    }

    /// Invia un pacchetto IP generato localmente, scegliendo la rotta.
    /// Se l'indirizzo sorgente è 0.0.0.0 viene usato quello dell'interfaccia d'uscita.
    fn send_ip(&mut self, dev_id: DeviceId, mut packet: Ipv4Packet, pdu: u64) -> bool {
        match self.devices[&dev_id].lookup_route(packet.dst) {
            Some((iface, nh)) => {
                if packet.src.is_unspecified() {
                    packet.src = self.devices[&dev_id].interfaces[iface].ip.map(|c| c.addr).unwrap_or(packet.src);
                }
                self.transmit_ip(dev_id, iface, nh, packet, pdu);
                true
            }
            None => {
                let dst = packet.dst;
                self.log(dev_id, EventKind::Dropped, format!("nessuna rotta verso {dst}"), None);
                false
            }
        }
    }

    /// Incapsula in un frame verso `next_hop` (risolvendo con ARP se serve).
    fn transmit_ip(&mut self, dev_id: DeviceId, iface: usize, next_hop: Ipv4Addr, packet: Ipv4Packet, pdu: u64) {
        let dev = self.devices.get_mut(&dev_id).unwrap();
        let ifc = &dev.interfaces[iface];
        let src_mac = ifc.mac;
        let Some(my_ip) = ifc.ip else {
            self.log(dev_id, EventKind::Dropped, "interfaccia senza IP".into(), None);
            return;
        };
        if let Some(&dst_mac) = dev.state.arp_table.get(&next_hop) {
            let frame = Frame { pdu_id: pdu, src: src_mac, dst: dst_mac, vlan: None, payload: EtherPayload::Ipv4(packet) };
            self.emit(dev_id, iface, frame);
            return;
        }
        let already_asking = dev.state.pending.iter().any(|p| p.next_hop == next_hop);
        let tick = self.tick;
        dev.state.pending.push(PendingPacket { iface, next_hop, packet, pdu_id: pdu, since_tick: tick });
        if already_asking {
            return;
        }
        let arp_pdu = self.new_pdu();
        let req = Frame {
            pdu_id: arp_pdu,
            src: src_mac,
            dst: Mac::BROADCAST,
            vlan: None,
            payload: EtherPayload::Arp(ArpPacket {
                op: ArpOp::Request,
                sender_mac: src_mac,
                sender_ip: my_ip.addr,
                target_mac: Mac::ZERO,
                target_ip: next_hop,
            }),
        };
        self.log(dev_id, EventKind::Created, format!("MAC di {next_hop} sconosciuto: ARP request"), Some(&req));
        self.emit(dev_id, iface, req);
    }

    fn send_dhcp_broadcast(&mut self, dev_id: DeviceId, iface: usize, src: Ipv4Addr, sport: u16, dport: u16, msg: DhcpMessage) {
        let mac = self.devices[&dev_id].interfaces[iface].mac;
        let pdu = self.new_pdu();
        let frame = Frame {
            pdu_id: pdu,
            src: mac,
            dst: Mac::BROADCAST,
            vlan: None,
            payload: EtherPayload::Ipv4(Ipv4Packet {
                src,
                dst: Ipv4Addr::BROADCAST,
                ttl: DEFAULT_TTL_HOST,
                payload: L4::Udp(UdpDatagram { src_port: sport, dst_port: dport, payload: AppData::Dhcp(msg) }),
            }),
        };
        self.emit(dev_id, iface, frame);
    }

    fn flush_pending(&mut self, dev_id: DeviceId, resolved: Ipv4Addr) {
        let dev = self.devices.get_mut(&dev_id).unwrap();
        let (ready, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut dev.state.pending).into_iter().partition(|p| p.next_hop == resolved);
        dev.state.pending = rest;
        for p in ready {
            self.transmit_ip(dev_id, p.iface, p.next_hop, p.packet, p.pdu_id);
        }
    }

    fn expire_pending_arp(&mut self) {
        let now = self.tick;
        let mut expired = Vec::new();
        for d in self.devices.values_mut() {
            let (old, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut d.state.pending)
                .into_iter()
                .partition(|p| now.saturating_sub(p.since_tick) > ARP_TIMEOUT_TICKS);
            d.state.pending = keep;
            expired.extend(old.into_iter().map(|p| (d.id, p)));
        }
        for (dev, p) in expired {
            self.log(dev, EventKind::Dropped, format!("ARP per {} senza risposta", p.next_hop), None);
            if let Some(i) = p.packet.icmp() {
                if i.kind == IcmpKind::EchoRequest && p.packet.src == self.ip_of_iface(dev, p.iface) {
                    self.fail_ping(dev, i.id, i.seq, "Destination host unreachable", None);
                }
            }
        }
    }

    fn ip_of_iface(&self, dev: DeviceId, iface: usize) -> Ipv4Addr {
        self.devices[&dev].interfaces[iface].ip.map(|c| c.addr).unwrap_or(Ipv4Addr::UNSPECIFIED)
    }

    fn fail_ping(&mut self, src: DeviceId, id: u16, seq: u16, reason: &str, from: Option<Ipv4Addr>) {
        if let Some(p) = self
            .pings
            .iter_mut()
            .find(|p| p.src == src && p.id == id && p.seq == seq && p.status == PingStatus::Pending)
        {
            p.status = PingStatus::Failed(reason.to_string());
            p.reply_from = from;
            let dst = p.dst;
            self.log(src, EventKind::Failure, format!("Ping verso {dst}: {reason}"), None);
        }
    }

    fn local_deliver(&mut self, to: Endpoint, p: Ipv4Packet, frame: &Frame) {
        let dev_id = to.device;
        match &p.payload {
            L4::Icmp(icmp) => {
                self.log(dev_id, EventKind::Received, format!("ICMP {} da {}", icmp.kind.label(), p.src), Some(frame));
                self.icmp_local(dev_id, &p, icmp.clone());
            }
            L4::Udp(u) => match (&u.payload, u.dst_port) {
                (AppData::Dhcp(m), PORT_DHCP_SERVER) => self.dhcp_port67(to, &p, m.clone(), frame),
                (AppData::Dhcp(m), PORT_DHCP_CLIENT) => self.dhcp_client_receive(to, m.clone(), frame),
                (AppData::Dns(m), PORT_DNS) if !m.response => self.dns_server_receive(dev_id, &p, u, m.clone(), frame),
                (AppData::Dns(m), _) if m.response => self.dns_client_receive(dev_id, m.clone(), frame),
                _ => self.log(dev_id, EventKind::Dropped, format!("porta UDP {} chiusa", u.dst_port), Some(frame)),
            },
        }
    }

    fn icmp_local(&mut self, dev_id: DeviceId, p: &Ipv4Packet, icmp: IcmpPacket) {
        match icmp.kind {
            IcmpKind::EchoRequest => {
                if p.dst == Ipv4Addr::BROADCAST {
                    return;
                }
                let ttl = if self.devices[&dev_id].kind.is_host() { DEFAULT_TTL_HOST } else { DEFAULT_TTL_ROUTER };
                let reply = Ipv4Packet {
                    src: p.dst,
                    dst: p.src,
                    ttl,
                    payload: L4::Icmp(IcmpPacket { kind: IcmpKind::EchoReply, id: icmp.id, seq: icmp.seq }),
                };
                let pdu = self.new_pdu();
                self.send_ip(dev_id, reply, pdu);
            }
            IcmpKind::EchoReply => {
                let tick = self.tick;
                if let Some(rec) = self
                    .pings
                    .iter_mut()
                    .find(|r| r.src == dev_id && r.id == icmp.id && r.seq == icmp.seq && r.status == PingStatus::Pending)
                {
                    rec.status = PingStatus::Success { ticks: tick - rec.sent_tick, ttl: p.ttl };
                    rec.reply_from = Some(p.src);
                    self.log(dev_id, EventKind::Success, format!("Reply da {}: TTL={}", p.src, p.ttl), None);
                }
            }
            IcmpKind::DestUnreachable => {
                self.fail_ping(dev_id, icmp.id, icmp.seq, "Destination host unreachable", Some(p.src));
            }
            IcmpKind::TimeExceeded => {
                self.fail_ping(dev_id, icmp.id, icmp.seq, "TTL expired in transit", Some(p.src));
            }
        }
    }

    /// Inoltro di un pacchetto da parte di un router.
    fn forward(&mut self, from: Endpoint, mut p: Ipv4Packet, frame: &Frame) {
        if p.ttl <= 1 {
            self.log(from.device, EventKind::Dropped, "TTL scaduto".into(), Some(frame));
            self.icmp_error(from, &p, IcmpKind::TimeExceeded);
            return;
        }
        p.ttl -= 1;
        match self.devices[&from.device].lookup_route(p.dst) {
            Some((iface, nh)) => {
                let short = short_iface_name(&self.devices[&from.device].interfaces[iface].name);
                self.log(from.device, EventKind::Received, format!("instrada {} via {short} (next hop {nh})", p.dst), Some(frame));
                self.transmit_ip(from.device, iface, nh, p, frame.pdu_id);
            }
            None => {
                self.log(from.device, EventKind::Dropped, format!("nessuna rotta verso {}", p.dst), Some(frame));
                self.icmp_error(from, &p, IcmpKind::DestUnreachable);
            }
        }
    }

    /// Genera un errore ICMP verso il mittente (mai in risposta a un altro errore).
    fn icmp_error(&mut self, at: Endpoint, original: &Ipv4Packet, kind: IcmpKind) {
        let Some(orig) = original.icmp() else { return };
        if orig.kind.is_error() {
            return;
        }
        let Some(src_ip) = self.devices[&at.device].interfaces[at.iface].ip.map(|c| c.addr) else { return };
        let err = Ipv4Packet {
            src: src_ip,
            dst: original.src,
            ttl: DEFAULT_TTL_ROUTER,
            payload: L4::Icmp(IcmpPacket { kind, id: orig.id, seq: orig.seq }),
        };
        let pdu = self.new_pdu();
        self.send_ip(at.device, err, pdu);
    }

    // ------------------------------------------------------------------
    // DHCP
    // ------------------------------------------------------------------

    fn dhcp_port67(&mut self, to: Endpoint, p: &Ipv4Packet, msg: DhcpMessage, frame: &Frame) {
        let dev = &self.devices[&to.device];
        match msg.op {
            DhcpOp::Discover | DhcpOp::Request => {
                let key = if msg.relay.is_unspecified() {
                    dev.interfaces[to.iface].ip.map(|c| c.addr)
                } else {
                    Some(msg.relay)
                };
                let pool = key.and_then(|k| {
                    dev.dhcp_pools.iter().position(|pl| pl.network.is_some_and(|n| n.contains(k)))
                });
                if dev.serves_dhcp() && pool.is_some() {
                    self.dhcp_serve(to, pool.unwrap(), msg, frame);
                } else if let (true, Some(helper), Some(myip)) =
                    (dev.kind.is_router(), dev.interfaces[to.iface].helper, dev.interfaces[to.iface].ip)
                {
                    if !msg.relay.is_unspecified() {
                        return;
                    }
                    self.log(to.device, EventKind::Received, format!("relay DHCP {} verso {helper}", msg.op.label()), Some(frame));
                    let mut m = msg;
                    m.relay = myip.addr;
                    let pkt = Ipv4Packet {
                        src: myip.addr,
                        dst: helper,
                        ttl: DEFAULT_TTL_ROUTER,
                        payload: L4::Udp(UdpDatagram {
                            src_port: PORT_DHCP_SERVER,
                            dst_port: PORT_DHCP_SERVER,
                            payload: AppData::Dhcp(m),
                        }),
                    };
                    self.send_ip(to.device, pkt, frame.pdu_id);
                } else {
                    self.log(to.device, EventKind::Dropped, "DHCP: non sono un server per questa rete".into(), Some(frame));
                }
            }
            DhcpOp::Offer | DhcpOp::Ack | DhcpOp::Nak => {
                // Risposta del server a un relay: la rimandiamo in broadcast sulla LAN del client.
                let relay_iface = dev.interfaces.iter().position(|i| i.ip.map(|c| c.addr) == Some(msg.relay));
                match (dev.kind.is_router(), relay_iface, p.dst == msg.relay) {
                    (true, Some(ri), true) => {
                        self.log(to.device, EventKind::Received, format!("relay DHCP {} verso il client", msg.op.label()), Some(frame));
                        self.send_dhcp_broadcast(to.device, ri, msg.relay, PORT_DHCP_SERVER, PORT_DHCP_CLIENT, msg);
                    }
                    _ => self.log(to.device, EventKind::Dropped, "risposta DHCP inattesa".into(), Some(frame)),
                }
            }
        }
    }

    fn dhcp_serve(&mut self, to: Endpoint, pool_idx: usize, msg: DhcpMessage, frame: &Frame) {
        let dev = &self.devices[&to.device];
        let Some(my_ip) = dev.interfaces[to.iface].ip.map(|c| c.addr) else { return };
        let pool = dev.dhcp_pools[pool_idx].clone();
        let net = pool.network.unwrap();
        let in_use = |ip: Ipv4Addr| {
            dev.is_excluded(ip)
                || dev.interfaces.iter().any(|i| i.ip.map(|c| c.addr) == Some(ip))
                || pool.default_router == Some(ip)
                || dev.dhcp_leases.iter().any(|l| l.ip == ip && l.mac != msg.client_mac)
        };
        let existing = dev.dhcp_leases.iter().find(|l| l.mac == msg.client_mac && net.contains(l.ip)).map(|l| l.ip);

        let (reply_op, ip) = match msg.op {
            DhcpOp::Discover => {
                let ip = existing.or_else(|| pool.candidates().into_iter().find(|ip| !in_use(*ip)));
                match ip {
                    Some(ip) => (DhcpOp::Offer, ip),
                    None => {
                        self.log(to.device, EventKind::Dropped, format!("pool DHCP '{}' esaurito", pool.name), Some(frame));
                        return;
                    }
                }
            }
            _ => {
                if msg.server_ip != my_ip {
                    self.log(to.device, EventKind::Dropped, "REQUEST per un altro server".into(), Some(frame));
                    return;
                }
                if net.contains(msg.your_ip) && !in_use(msg.your_ip) {
                    (DhcpOp::Ack, msg.your_ip)
                } else {
                    (DhcpOp::Nak, Ipv4Addr::UNSPECIFIED)
                }
            }
        };
        if reply_op == DhcpOp::Ack {
            let dev = self.devices.get_mut(&to.device).unwrap();
            dev.dhcp_leases.retain(|l| l.mac != msg.client_mac);
            dev.dhcp_leases.push(DhcpLease { mac: msg.client_mac, ip });
        }
        self.log(to.device, EventKind::Received, format!("DHCP {}: rispondo {} {ip}", msg.op.label(), reply_op.label()), Some(frame));
        let reply = DhcpMessage {
            op: reply_op,
            xid: msg.xid,
            client_mac: msg.client_mac,
            your_ip: ip,
            server_ip: my_ip,
            prefix: net.prefix,
            router: pool.default_router,
            dns: pool.dns,
            relay: msg.relay,
        };
        if msg.relay.is_unspecified() {
            self.send_dhcp_broadcast(to.device, to.iface, my_ip, PORT_DHCP_SERVER, PORT_DHCP_CLIENT, reply);
        } else {
            let pkt = Ipv4Packet {
                src: my_ip,
                dst: msg.relay,
                ttl: DEFAULT_TTL_HOST,
                payload: L4::Udp(UdpDatagram {
                    src_port: PORT_DHCP_SERVER,
                    dst_port: PORT_DHCP_SERVER,
                    payload: AppData::Dhcp(reply),
                }),
            };
            let pdu = self.new_pdu();
            self.send_ip(to.device, pkt, pdu);
        }
    }

    fn dhcp_client_receive(&mut self, to: Endpoint, msg: DhcpMessage, frame: &Frame) {
        let dev = self.devices.get_mut(&to.device).unwrap();
        let mine = dev.interfaces[to.iface].mac == msg.client_mac;
        match (dev.state.dhcp.clone(), msg.op) {
            (DhcpClientState::Discovering { xid, iface }, DhcpOp::Offer) if xid == msg.xid && mine => {
                dev.state.dhcp = DhcpClientState::Requesting { xid, iface };
                self.log(to.device, EventKind::Received, format!("DHCP OFFER {} da {}", msg.your_ip, msg.server_ip), Some(frame));
                let req = DhcpMessage { op: DhcpOp::Request, relay: Ipv4Addr::UNSPECIFIED, ..msg };
                self.send_dhcp_broadcast(to.device, iface, Ipv4Addr::UNSPECIFIED, PORT_DHCP_CLIENT, PORT_DHCP_SERVER, req);
            }
            (DhcpClientState::Requesting { xid, iface }, DhcpOp::Ack) if xid == msg.xid && mine => {
                dev.state.dhcp = DhcpClientState::Bound;
                dev.interfaces[iface].ip = Some(Ipv4Cidr::new(msg.your_ip, msg.prefix));
                dev.gateway = msg.router;
                if msg.dns.is_some() {
                    dev.dns_server = msg.dns;
                }
                self.log(to.device, EventKind::Success, format!("DHCP ACK: ottenuto {}/{}", msg.your_ip, msg.prefix), Some(frame));
            }
            (DhcpClientState::Requesting { xid, .. }, DhcpOp::Nak) if xid == msg.xid && mine => {
                self.log(to.device, EventKind::Received, "DHCP NAK".into(), Some(frame));
                self.dhcp_fail(to.device);
            }
            _ => self.log(to.device, EventKind::Dropped, "messaggio DHCP non per me".into(), Some(frame)),
        }
    }

    // ------------------------------------------------------------------
    // DNS
    // ------------------------------------------------------------------

    fn dns_server_receive(&mut self, dev_id: DeviceId, p: &Ipv4Packet, u: &UdpDatagram, m: DnsMessage, frame: &Frame) {
        let dev = &self.devices[&dev_id];
        if !dev.services.dns {
            self.log(dev_id, EventKind::Dropped, "servizio DNS spento".into(), Some(frame));
            return;
        }
        let answer = dev.dns_records.iter().find(|(n, _)| n.eq_ignore_ascii_case(&m.name)).map(|(_, ip)| *ip);
        self.log(dev_id, EventKind::Received, format!("DNS query {}", m.name), Some(frame));
        let reply = Ipv4Packet {
            src: p.dst,
            dst: p.src,
            ttl: DEFAULT_TTL_HOST,
            payload: L4::Udp(UdpDatagram {
                src_port: PORT_DNS,
                dst_port: u.src_port,
                payload: AppData::Dns(DnsMessage { id: m.id, response: true, name: m.name, answer }),
            }),
        };
        let pdu = self.new_pdu();
        self.send_ip(dev_id, reply, pdu);
    }

    fn dns_client_receive(&mut self, dev_id: DeviceId, m: DnsMessage, frame: &Frame) {
        if let Some(l) = self
            .dns_lookups
            .iter_mut()
            .find(|l| l.device == dev_id && l.id == m.id && l.status == DnsStatus::Pending)
        {
            l.status = match m.answer {
                Some(ip) => DnsStatus::Resolved(ip),
                None => DnsStatus::NotFound,
            };
            self.log(dev_id, EventKind::Success, format!("DNS: {} = {:?}", m.name, m.answer), Some(frame));
        }
    }

    // ------------------------------------------------------------------
    // Persistenza
    // ------------------------------------------------------------------

    pub fn to_json(&self) -> Result<String, String> {
        let file = SaveFile { format: SAVE_FORMAT.into(), version: SAVE_VERSION, network: self.clone() };
        serde_json::to_string_pretty(&file).map_err(|e| e.to_string())
    }

    pub fn from_json(s: &str) -> Result<Network, String> {
        let file: SaveFile = serde_json::from_str(s).map_err(|e| format!("file non valido: {e}"))?;
        if file.format != SAVE_FORMAT {
            return Err("non è un file NetPackeT".into());
        }
        if file.version > SAVE_VERSION {
            return Err(format!("file creato con una versione più recente ({})", file.version));
        }
        Ok(file.network)
    }
}

pub const SAVE_FORMAT: &str = "netpacket";
pub const SAVE_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct SaveFile {
    format: String,
    version: u32,
    network: Network,
}

/// Verifica che un indirizzo sia assegnabile a un'interfaccia.
pub fn validate_host_ip(c: Ipv4Cidr) -> Result<(), String> {
    if c.addr.is_unspecified() || c.addr.is_broadcast() || c.addr.is_multicast() {
        return Err(format!("{} non è un indirizzo host valido", c.addr));
    }
    if c.prefix == 0 {
        return Err("prefisso /0 non valido per un'interfaccia".into());
    }
    if c.prefix < 31 && (c.addr == c.network() || c.addr == c.broadcast()) {
        return Err(format!("{} è l'indirizzo di rete o di broadcast della subnet", c.addr));
    }
    Ok(())
}
