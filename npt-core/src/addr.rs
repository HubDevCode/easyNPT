//! Indirizzi di livello 2 (MAC) e livello 3 (IPv4 con prefisso).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

/// Indirizzo MAC a 48 bit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug)]
pub struct Mac(pub [u8; 6]);

impl Mac {
    pub const BROADCAST: Mac = Mac([0xff; 6]);
    pub const ZERO: Mac = Mac([0; 6]);

    pub fn is_broadcast(&self) -> bool {
        *self == Self::BROADCAST
    }

    /// Genera un MAC "amministrato localmente" a partire da un contatore.
    pub fn from_counter(n: u32) -> Mac {
        let b = n.to_be_bytes();
        Mac([0x02, 0x4e, b[0], b[1], b[2], b[3]])
    }
}

impl fmt::Display for Mac {
    /// Formato a gruppi di 4 cifre esadecimali: `024e.0000.0001`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.0;
        write!(
            f,
            "{:02x}{:02x}.{:02x}{:02x}.{:02x}{:02x}",
            m[0], m[1], m[2], m[3], m[4], m[5]
        )
    }
}

/// Indirizzo IPv4 con lunghezza del prefisso (es. 192.168.1.10/24).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub struct Ipv4Cidr {
    pub addr: Ipv4Addr,
    pub prefix: u8,
}

impl Ipv4Cidr {
    pub fn new(addr: Ipv4Addr, prefix: u8) -> Self {
        Self { addr, prefix: prefix.min(32) }
    }

    pub fn mask_u32(&self) -> u32 {
        prefix_to_mask(self.prefix)
    }

    pub fn mask(&self) -> Ipv4Addr {
        Ipv4Addr::from(self.mask_u32())
    }

    pub fn network(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.addr) & self.mask_u32())
    }

    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.addr) | !self.mask_u32())
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        (u32::from(ip) & self.mask_u32()) == (u32::from(self.addr) & self.mask_u32())
    }

    /// Restituisce la stessa rete con l'indirizzo normalizzato (bit host a zero).
    pub fn as_network(&self) -> Ipv4Cidr {
        Ipv4Cidr::new(self.network(), self.prefix)
    }
}

impl fmt::Display for Ipv4Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

impl FromStr for Ipv4Cidr {
    type Err = String;
    /// Accetta `a.b.c.d/nn`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (ip, pfx) = s
            .trim()
            .split_once('/')
            .ok_or_else(|| format!("formato non valido: '{s}' (atteso a.b.c.d/nn)"))?;
        let addr: Ipv4Addr = ip.trim().parse().map_err(|_| format!("IP non valido: '{ip}'"))?;
        let prefix: u8 = pfx.trim().parse().map_err(|_| format!("prefisso non valido: '{pfx}'"))?;
        if prefix > 32 {
            return Err(format!("prefisso fuori intervallo: {prefix}"));
        }
        Ok(Ipv4Cidr::new(addr, prefix))
    }
}

pub fn prefix_to_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix.min(32) as u32)
    }
}

/// Converte una subnet mask puntata (255.255.255.0) in lunghezza del prefisso.
/// Restituisce errore se la maschera non è contigua.
pub fn mask_to_prefix(mask: Ipv4Addr) -> Result<u8, String> {
    let m = u32::from(mask);
    let ones = m.leading_ones();
    if m.checked_shl(ones).unwrap_or(0) != 0 {
        return Err(format!("subnet mask non contigua: {mask}"));
    }
    Ok(ones as u8)
}

/// Prefisso "classful" suggerito per un indirizzo (classe A/B/C).
pub fn default_prefix_for(ip: Ipv4Addr) -> u8 {
    match ip.octets()[0] {
        0..=127 => 8,
        128..=191 => 16,
        _ => 24,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_roundtrip() {
        for p in 0..=32u8 {
            let m = Ipv4Addr::from(prefix_to_mask(p));
            assert_eq!(mask_to_prefix(m).unwrap(), p);
        }
        assert!(mask_to_prefix("255.0.255.0".parse().unwrap()).is_err());
    }

    #[test]
    fn cidr_contains() {
        let c: Ipv4Cidr = "192.168.1.10/24".parse().unwrap();
        assert!(c.contains("192.168.1.200".parse().unwrap()));
        assert!(!c.contains("192.168.2.1".parse().unwrap()));
        assert_eq!(c.network(), "192.168.1.0".parse::<Ipv4Addr>().unwrap());
        assert_eq!(c.broadcast(), "192.168.1.255".parse::<Ipv4Addr>().unwrap());
    }
}
