//! Motore di simulazione a eventi discreti.
//!
//! Il tempo avanza a "tick": a ogni tick tutti i frame in transito su un cavo
//! arrivano al dispositivo all'altra estremità, che li elabora e può generarne
//! di nuovi (in partenza al tick successivo).

use crate::addr::{Ipv4Cidr, Mac};
use crate::device::{short_iface_name, Device, DeviceId, DeviceKind, LinkId, PendingPacket};
use crate::packet::{ArpOp, ArpPacket, EtherPayload, Frame, IcmpKind, IcmpPacket, Ipv4Packet, L4};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

/// Dopo quanti tick una richiesta ARP senza risposta viene abbandonata.
pub const ARP_TIMEOUT_TICKS: u64 = 12;
pub const DEFAULT_TTL_HOST: u8 = 128;
pub const DEFAULT_TTL_ROUTER: u8 = 255;

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
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Network {
    pub devices: BTreeMap<DeviceId, Device>,
    pub links: BTreeMap<LinkId, Link>,
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
    next_pdu: u64,
    #[serde(skip)]
    ping_seq: u16,
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
        let links: Vec<LinkId> = self
            .links
            .values()
            .filter(|l| l.a.device == id || l.b.device == id)
            .map(|l| l.id)
            .collect();
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

    /// Collega due interfacce specifiche.
    pub fn connect(&mut self, a: Endpoint, b: Endpoint) -> Result<LinkId, String> {
        if a.device == b.device {
            return Err("non si può collegare un dispositivo a se stesso".into());
        }
        for ep in [a, b] {
            let dev = self.devices.get(&ep.device).ok_or("dispositivo inesistente")?;
            let iface = dev.interfaces.get(ep.iface).ok_or("interfaccia inesistente")?;
            if iface.link.is_some() {
                return Err(format!("{} {} è già collegata", dev.name, iface.name));
            }
        }
        let kind_a = self.devices[&a.device].kind;
        let kind_b = self.devices[&b.device].kind;
        let cable = if same_mdi_class(kind_a, kind_b) {
            CableKind::CopperCross
        } else {
            CableKind::CopperStraight
        };
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
            self.devices
                .get(&ep.device)
                .and_then(|d| d.interfaces.get(ep.iface))
                .is_some_and(|i| i.enabled)
        })
    }

    /// Descrive l'estremità opposta di un'interfaccia: "Switch0 Fa0/3".
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
            self.finalize_pings();
        }
    }

    /// Esegue tick finché la rete non è inattiva (o si raggiunge il limite).
    pub fn run_until_idle(&mut self, max_ticks: u64) {
        let start = self.tick;
        while !self.is_idle() && self.tick - start < max_ticks {
            self.step();
        }
        self.finalize_pings();
    }

    /// I ping ancora in attesa a rete inattiva sono andati persi.
    fn finalize_pings(&mut self) {
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
    }

    fn log(&mut self, device: DeviceId, kind: EventKind, info: String, frame: Option<&Frame>) {
        self.events.push(SimEvent { tick: self.tick, device, kind, info, frame: frame.cloned() });
    }

    fn new_pdu(&mut self) -> u64 {
        self.next_pdu += 1;
        self.next_pdu
    }

    // ------------------------------------------------------------------
    // Comandi utente
    // ------------------------------------------------------------------

    /// Invia un ICMP Echo Request da `src` verso `dst`.
    pub fn ping(&mut self, src: DeviceId, dst: Ipv4Addr) -> Result<(), String> {
        let dev = self.devices.get(&src).ok_or("dispositivo inesistente")?;
        if dev.kind.is_l2() {
            return Err(format!("{} non ha un indirizzo IP (dispositivo di livello 2)", dev.name));
        }
        self.ping_seq = self.ping_seq.wrapping_add(1);
        let seq = self.ping_seq;
        let id = src as u16;
        let ttl = if dev.kind.is_router() { DEFAULT_TTL_ROUTER } else { DEFAULT_TTL_HOST };
        let owns = dev.owns_ip(dst);
        let route = dev.lookup_route(dst);
        let src_name = dev.name.clone();

        let mut rec = PingRecord { src, dst, id, seq, sent_tick: self.tick, status: PingStatus::Pending };
        if owns {
            rec.status = PingStatus::Success { ticks: 0, ttl };
            self.pings.push(rec);
            self.log(src, EventKind::Success, format!("Ping verso se stesso ({dst})"), None);
            return Ok(());
        }
        let Some((iface, next_hop)) = route else {
            rec.status = PingStatus::Failed("nessuna rotta verso la destinazione".into());
            self.pings.push(rec);
            self.log(src, EventKind::Failure, format!("{src_name}: nessuna rotta verso {dst}"), None);
            return Ok(());
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
        Ok(())
    }

    // ------------------------------------------------------------------
    // Livello fisico / collegamento
    // ------------------------------------------------------------------

    /// Mette un frame sul cavo collegato all'interfaccia.
    fn emit(&mut self, dev_id: DeviceId, iface: usize, frame: Frame) {
        let dev = &self.devices[&dev_id];
        let ifc = &dev.interfaces[iface];
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
            _ => self.l3_receive(to, frame),
        }
    }

    fn hub_receive(&mut self, to: Endpoint, frame: Frame) {
        self.log(to.device, EventKind::Received, "ripete su tutte le altre porte".into(), Some(&frame));
        let ports: Vec<usize> = self.connected_ports_except(to.device, to.iface);
        for p in ports {
            self.emit(to.device, p, frame.clone());
        }
    }

    fn switch_receive(&mut self, to: Endpoint, frame: Frame) {
        let dev = self.devices.get_mut(&to.device).unwrap();
        // Apprendimento dell'indirizzo sorgente.
        if !frame.src.is_broadcast() {
            dev.state.mac_table.insert(frame.src, to.iface);
        }
        let known = if frame.dst.is_broadcast() { None } else { dev.state.mac_table.get(&frame.dst).copied() };
        match known {
            Some(port) if port == to.iface => {
                self.log(to.device, EventKind::Dropped, "destinazione sulla stessa porta: filtrato".into(), Some(&frame));
            }
            Some(port) => {
                let short = short_iface_name(&self.devices[&to.device].interfaces[port].name);
                self.log(to.device, EventKind::Received, format!("MAC {} noto su {short}: inoltra", frame.dst), Some(&frame));
                self.emit(to.device, port, frame);
            }
            None => {
                let why = if frame.dst.is_broadcast() { "broadcast" } else { "MAC sconosciuto" };
                self.log(to.device, EventKind::Received, format!("{why}: flooding"), Some(&frame));
                for p in self.connected_ports_except(to.device, to.iface) {
                    self.emit(to.device, p, frame.clone());
                }
            }
        }
    }

    fn connected_ports_except(&self, dev: DeviceId, except: usize) -> Vec<usize> {
        self.devices[&dev]
            .interfaces
            .iter()
            .enumerate()
            .filter(|(i, ifc)| *i != except && ifc.link.is_some() && ifc.enabled)
            .map(|(i, _)| i)
            .collect()
    }

    // ------------------------------------------------------------------
    // Livello 3 (terminali e router)
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
                    self.log(to.device, EventKind::Received, format!("pacchetto per me da {}", p.src), Some(&frame));
                    self.local_deliver(to.device, p);
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
        // Aggiorna la cache se il mittente è già noto o se la richiesta è per noi.
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
    fn send_ip(&mut self, dev_id: DeviceId, packet: Ipv4Packet, pdu: u64) {
        match self.devices[&dev_id].lookup_route(packet.dst) {
            Some((iface, nh)) => self.transmit_ip(dev_id, iface, nh, packet, pdu),
            None => {
                let dst = packet.dst;
                self.log(dev_id, EventKind::Dropped, format!("nessuna rotta verso {dst}"), None);
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
            let frame = Frame { pdu_id: pdu, src: src_mac, dst: dst_mac, payload: EtherPayload::Ipv4(packet) };
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
            let L4::Icmp(i) = &p.packet.payload;
            if i.kind == IcmpKind::EchoRequest {
                self.fail_ping(dev, i.id, i.seq, "Destination host unreachable (ARP)");
            }
        }
    }

    fn fail_ping(&mut self, src: DeviceId, id: u16, seq: u16, reason: &str) {
        if let Some(p) = self
            .pings
            .iter_mut()
            .find(|p| p.src == src && p.id == id && p.seq == seq && p.status == PingStatus::Pending)
        {
            p.status = PingStatus::Failed(reason.to_string());
            let dst = p.dst;
            self.log(src, EventKind::Failure, format!("Ping verso {dst}: {reason}"), None);
        }
    }

    fn local_deliver(&mut self, dev_id: DeviceId, p: Ipv4Packet) {
        let L4::Icmp(icmp) = &p.payload;
        match icmp.kind {
            IcmpKind::EchoRequest => {
                let ttl = if self.devices[&dev_id].kind.is_router() { DEFAULT_TTL_ROUTER } else { DEFAULT_TTL_HOST };
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
                if let Some(rec) = self.pings.iter_mut().find(|r| {
                    r.src == dev_id && r.id == icmp.id && r.seq == icmp.seq && r.status == PingStatus::Pending
                }) {
                    rec.status = PingStatus::Success { ticks: tick - rec.sent_tick, ttl: p.ttl };
                    self.log(dev_id, EventKind::Success, format!("Reply da {}: TTL={}", p.src, p.ttl), None);
                }
            }
            IcmpKind::DestUnreachable => {
                let reason = format!("Reply da {}: Destination host unreachable", p.src);
                self.fail_ping(dev_id, icmp.id, icmp.seq, &reason);
            }
            IcmpKind::TimeExceeded => {
                let reason = format!("Reply da {}: TTL expired in transit", p.src);
                self.fail_ping(dev_id, icmp.id, icmp.seq, &reason);
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
        let L4::Icmp(orig) = &original.payload;
        if !matches!(orig.kind, IcmpKind::EchoRequest | IcmpKind::EchoReply) {
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

/// Due dispositivi della stessa "classe" (MDI vs MDI-X) richiedono un cavo incrociato.
fn same_mdi_class(a: DeviceKind, b: DeviceKind) -> bool {
    a.is_l2() == b.is_l2()
}

pub const SAVE_FORMAT: &str = "netpacket";
pub const SAVE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SaveFile {
    format: String,
    version: u32,
    network: Network,
}

/// Helper per assegnare un IP a un'interfaccia con validazione.
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
