//! Rappresentazione delle PDU che viaggiano sulla rete simulata.

use crate::addr::Mac;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const PORT_DHCP_SERVER: u16 = 67;
pub const PORT_DHCP_CLIENT: u16 = 68;
pub const PORT_DNS: u16 = 53;

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
            IcmpKind::DestUnreachable => (3, 1),
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

    pub fn is_error(&self) -> bool {
        matches!(self, IcmpKind::DestUnreachable | IcmpKind::TimeExceeded)
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct IcmpPacket {
    pub kind: IcmpKind,
    pub id: u16,
    pub seq: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DhcpOp {
    Discover,
    Offer,
    Request,
    Ack,
    Nak,
}

impl DhcpOp {
    pub fn label(&self) -> &'static str {
        match self {
            DhcpOp::Discover => "DISCOVER",
            DhcpOp::Offer => "OFFER",
            DhcpOp::Request => "REQUEST",
            DhcpOp::Ack => "ACK",
            DhcpOp::Nak => "NAK",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DhcpMessage {
    pub op: DhcpOp,
    pub xid: u32,
    pub client_mac: Mac,
    /// Indirizzo offerto/assegnato al client ("yiaddr").
    pub your_ip: Ipv4Addr,
    /// Server che ha fatto l'offerta.
    pub server_ip: Ipv4Addr,
    pub prefix: u8,
    pub router: Option<Ipv4Addr>,
    pub dns: Option<Ipv4Addr>,
    /// Indirizzo del relay agent ("giaddr"), 0.0.0.0 se assente.
    #[serde(default = "unspecified")]
    pub relay: Ipv4Addr,
}

fn unspecified() -> Ipv4Addr {
    Ipv4Addr::UNSPECIFIED
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DnsMessage {
    pub id: u16,
    pub response: bool,
    pub name: String,
    pub answer: Option<Ipv4Addr>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum AppData {
    Dhcp(DhcpMessage),
    Dns(DnsMessage),
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct UdpDatagram {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: AppData,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum L4 {
    Icmp(IcmpPacket),
    Udp(UdpDatagram),
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
            L4::Udp(_) => 17,
        }
    }

    pub fn icmp(&self) -> Option<&IcmpPacket> {
        match &self.payload {
            L4::Icmp(i) => Some(i),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum EtherPayload {
    Arp(ArpPacket),
    Ipv4(Ipv4Packet),
}

/// Frame Ethernet II, eventualmente con tag 802.1Q.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Frame {
    /// Identificativo della PDU "logica" (stesso valore lungo tutto il percorso di un pacchetto).
    pub pdu_id: u64,
    pub src: Mac,
    pub dst: Mac,
    #[serde(default)]
    pub vlan: Option<u16>,
    pub payload: EtherPayload,
}

/// Un livello del modello OSI con le sue righe descrittive (per l'ispettore PDU).
#[derive(Clone, Debug)]
pub struct LayerInfo {
    pub layer: u8,
    pub title: String,
    pub fields: Vec<(String, String)>,
}

fn opt_ip(v: Option<Ipv4Addr>) -> String {
    v.map(|i| i.to_string()).unwrap_or_else(|| "—".into())
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
            EtherPayload::Ipv4(ip) => match &ip.payload {
                L4::Icmp(_) => "ICMP",
                L4::Udp(u) => match u.payload {
                    AppData::Dhcp(_) => "DHCP",
                    AppData::Dns(_) => "DNS",
                },
            },
        }
    }

    /// Descrizione di una riga.
    pub fn summary(&self) -> String {
        let tag = self.vlan.map(|v| format!(" [VLAN {v}]")).unwrap_or_default();
        let s = match &self.payload {
            EtherPayload::Arp(a) => match a.op {
                ArpOp::Request => format!("ARP chi ha {}? Risponda a {}", a.target_ip, a.sender_ip),
                ArpOp::Reply => format!("ARP {} è a {}", a.sender_ip, a.sender_mac),
            },
            EtherPayload::Ipv4(ip) => match &ip.payload {
                L4::Icmp(i) => format!("ICMP {} {} → {} (seq {}, TTL {})", i.kind.label(), ip.src, ip.dst, i.seq, ip.ttl),
                L4::Udp(u) => match &u.payload {
                    AppData::Dhcp(d) => format!("DHCP {} xid {:08x} ip {}", d.op.label(), d.xid, d.your_ip),
                    AppData::Dns(d) if d.response => {
                        format!("DNS risposta {} = {}", d.name, opt_ip(d.answer))
                    }
                    AppData::Dns(d) => format!("DNS query {} → {}", d.name, ip.dst),
                },
            },
        };
        s + &tag
    }

    /// Scomposizione per livelli, dal 2 in su.
    pub fn layers(&self) -> Vec<LayerInfo> {
        let mut eth = vec![
            ("MAC destinazione".into(), self.dst.to_string()),
            ("MAC sorgente".into(), self.src.to_string()),
        ];
        if let Some(v) = self.vlan {
            eth.push(("802.1Q VLAN ID".into(), v.to_string()));
        }
        eth.push(("EtherType".into(), format!("0x{:04x}", self.ethertype())));
        let mut out = vec![LayerInfo { layer: 2, title: "Ethernet II".into(), fields: eth }];
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
                    L4::Udp(u) => {
                        out.push(LayerInfo {
                            layer: 4,
                            title: "UDP".into(),
                            fields: vec![
                                ("Porta sorgente".into(), u.src_port.to_string()),
                                ("Porta destinazione".into(), u.dst_port.to_string()),
                            ],
                        });
                        match &u.payload {
                            AppData::Dhcp(d) => out.push(LayerInfo {
                                layer: 7,
                                title: format!("DHCP {}", d.op.label()),
                                fields: vec![
                                    ("Transaction ID".into(), format!("0x{:08x}", d.xid)),
                                    ("MAC client".into(), d.client_mac.to_string()),
                                    ("IP assegnato (yiaddr)".into(), d.your_ip.to_string()),
                                    ("Server".into(), d.server_ip.to_string()),
                                    ("Relay (giaddr)".into(), d.relay.to_string()),
                                    ("Prefisso".into(), format!("/{}", d.prefix)),
                                    ("Router".into(), opt_ip(d.router)),
                                    ("DNS".into(), opt_ip(d.dns)),
                                ],
                            }),
                            AppData::Dns(d) => out.push(LayerInfo {
                                layer: 7,
                                title: if d.response { "DNS risposta".into() } else { "DNS query".into() },
                                fields: vec![
                                    ("ID".into(), d.id.to_string()),
                                    ("Nome".into(), d.name.clone()),
                                    ("Risposta (A)".into(), opt_ip(d.answer)),
                                ],
                            }),
                        }
                    }
                }
            }
        }
        out
    }
}
