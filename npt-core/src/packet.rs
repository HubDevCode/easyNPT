//! Rappresentazione delle PDU che viaggiano sulla rete simulata.

use crate::addr::Mac;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ArpOp {
    Request,
    Reply,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ArpPacket {
    pub op: ArpOp,
    pub sender_mac: Mac,
    pub sender_ip: Ipv4Addr,
    pub target_mac: Mac,
    pub target_ip: Ipv4Addr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum IcmpKind {
    EchoReply,
    DestUnreachable,
    EchoRequest,
    TimeExceeded,
}

impl IcmpKind {
    pub fn type_code(&self) -> (u8, u8) {
        match self {
            IcmpKind::EchoReply => (0, 0),
            IcmpKind::DestUnreachable => (3, 0),
            IcmpKind::EchoRequest => (8, 0),
            IcmpKind::TimeExceeded => (11, 0),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            IcmpKind::EchoReply => "Echo Reply",
            IcmpKind::DestUnreachable => "Destination Unreachable",
            IcmpKind::EchoRequest => "Echo Request",
            IcmpKind::TimeExceeded => "Time Exceeded",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct IcmpPacket {
    pub kind: IcmpKind,
    pub id: u16,
    pub seq: u16,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum L4 {
    Icmp(IcmpPacket),
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Ipv4Packet {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub ttl: u8,
    pub payload: L4,
}

impl Ipv4Packet {
    pub fn protocol_number(&self) -> u8 {
        match self.payload {
            L4::Icmp(_) => 1,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum EtherPayload {
    Arp(ArpPacket),
    Ipv4(Ipv4Packet),
}

/// Frame Ethernet II.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Frame {
    /// Identificativo della PDU "logica" (stesso valore lungo tutto il percorso di un pacchetto).
    pub pdu_id: u64,
    pub src: Mac,
    pub dst: Mac,
    pub payload: EtherPayload,
}

/// Un livello del modello OSI con le sue righe descrittive (per l'ispettore PDU).
#[derive(Clone, Debug)]
pub struct LayerInfo {
    pub layer: u8,
    pub title: String,
    pub fields: Vec<(String, String)>,
}

impl Frame {
    pub fn ethertype(&self) -> u16 {
        match self.payload {
            EtherPayload::Arp(_) => ETHERTYPE_ARP,
            EtherPayload::Ipv4(_) => ETHERTYPE_IPV4,
        }
    }

    /// Nome breve del protocollo di livello più alto.
    pub fn protocol(&self) -> &'static str {
        match &self.payload {
            EtherPayload::Arp(_) => "ARP",
            EtherPayload::Ipv4(ip) => match ip.payload {
                L4::Icmp(_) => "ICMP",
            },
        }
    }

    /// Descrizione di una riga.
    pub fn summary(&self) -> String {
        match &self.payload {
            EtherPayload::Arp(a) => match a.op {
                ArpOp::Request => format!("ARP chi ha {}? Risponda a {}", a.target_ip, a.sender_ip),
                ArpOp::Reply => format!("ARP {} è a {}", a.sender_ip, a.sender_mac),
            },
            EtherPayload::Ipv4(ip) => match &ip.payload {
                L4::Icmp(i) => format!(
                    "ICMP {} {} → {} (seq {}, TTL {})",
                    i.kind.label(),
                    ip.src,
                    ip.dst,
                    i.seq,
                    ip.ttl
                ),
            },
        }
    }

    /// Scomposizione per livelli, dal 2 in su.
    pub fn layers(&self) -> Vec<LayerInfo> {
        let mut out = vec![LayerInfo {
            layer: 2,
            title: "Ethernet II".into(),
            fields: vec![
                ("MAC destinazione".into(), self.dst.to_string()),
                ("MAC sorgente".into(), self.src.to_string()),
                ("EtherType".into(), format!("0x{:04x}", self.ethertype())),
            ],
        }];
        match &self.payload {
            EtherPayload::Arp(a) => out.push(LayerInfo {
                layer: 2,
                title: "ARP".into(),
                fields: vec![
                    (
                        "Operazione".into(),
                        match a.op {
                            ArpOp::Request => "1 (Request)".into(),
                            ArpOp::Reply => "2 (Reply)".into(),
                        },
                    ),
                    ("MAC mittente".into(), a.sender_mac.to_string()),
                    ("IP mittente".into(), a.sender_ip.to_string()),
                    ("MAC destinatario".into(), a.target_mac.to_string()),
                    ("IP destinatario".into(), a.target_ip.to_string()),
                ],
            }),
            EtherPayload::Ipv4(ip) => {
                out.push(LayerInfo {
                    layer: 3,
                    title: "IPv4".into(),
                    fields: vec![
                        ("IP sorgente".into(), ip.src.to_string()),
                        ("IP destinazione".into(), ip.dst.to_string()),
                        ("TTL".into(), ip.ttl.to_string()),
                        ("Protocollo".into(), ip.protocol_number().to_string()),
                    ],
                });
                match &ip.payload {
                    L4::Icmp(i) => {
                        let (t, c) = i.kind.type_code();
                        out.push(LayerInfo {
                            layer: 3,
                            title: "ICMP".into(),
                            fields: vec![
                                ("Tipo".into(), format!("{t} ({})", i.kind.label())),
                                ("Codice".into(), c.to_string()),
                                ("Identificatore".into(), i.id.to_string()),
                                ("Sequenza".into(), i.seq.to_string()),
                            ],
                        })
                    }
                }
            }
        }
        out
    }
}
