//! Prompt dei comandi dei terminali (PC, Laptop, Server).

use crate::device::DeviceId;
use crate::sim::{DnsStatus, Network, PingStatus, REALTIME_MAX_TICKS};
use std::fmt::Write as _;

pub const HOST_PROMPT: &str = "C:\\>";

const HELP: &str = "Comandi disponibili:
  arp -a             mostra la cache ARP
  arp -d             svuota la cache ARP
  ipconfig           mostra la configurazione IP
  ipconfig /all      configurazione completa (MAC, DNS)
  ipconfig /renew    richiede un indirizzo al server DHCP
  ipconfig /release  rilascia l'indirizzo DHCP
  nslookup <nome>    interroga il server DNS
  ping <ip|nome>     verifica la raggiungibilità (-n N per N pacchetti)
  tracert <ip|nome>  mostra il percorso verso una destinazione
  cls                pulisce lo schermo";

/// Esegue un comando del prompt e restituisce l'output.
pub fn run_host_command(net: &mut Network, dev: DeviceId, line: &str) -> String {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let Some(cmd) = toks.first() else { return String::new() };
    match cmd.to_ascii_lowercase().as_str() {
        "help" | "?" => HELP.into(),
        "ipconfig" => ipconfig(net, dev, toks.get(1).copied()),
        "ping" => ping(net, dev, &toks[1..]),
        "tracert" | "traceroute" => tracert(net, dev, toks.get(1).copied()),
        "arp" => arp(net, dev, toks.get(1).copied()),
        "nslookup" => nslookup(net, dev, toks.get(1).copied()),
        _ => format!("'{cmd}' non è riconosciuto come comando. Digita help per l'elenco."),
    }
}

fn ipconfig(net: &mut Network, dev: DeviceId, opt: Option<&str>) -> String {
    let opt = opt.map(|o| o.to_ascii_lowercase());
    match opt.as_deref() {
        Some("/renew") => {
            net.dhcp_request(dev, 0);
            net.run_until_idle(REALTIME_MAX_TICKS);
        }
        Some("/release") => {
            net.dhcp_release(dev, 0);
            return "Indirizzo IP rilasciato.".into();
        }
        _ => {}
    }
    let d = &net.devices[&dev];
    let mut s = String::new();
    for ifc in d.interfaces.iter().filter(|i| i.is_physical()) {
        let _ = writeln!(s, "\n{}:\n", ifc.name);
        if opt.as_deref() == Some("/all") {
            let _ = writeln!(s, "   Indirizzo fisico (MAC) . . . . . : {}", ifc.mac);
            let _ = writeln!(s, "   DHCP abilitato . . . . . . . . . : {}", if ifc.dhcp_client { "Sì" } else { "No" });
        }
        let _ = writeln!(s, "   Indirizzo IPv4 . . . . . . . . . : {}", ifc.ip.map(|c| c.addr.to_string()).unwrap_or("0.0.0.0".into()));
        let _ = writeln!(s, "   Subnet mask  . . . . . . . . . . : {}", ifc.ip.map(|c| c.mask().to_string()).unwrap_or("0.0.0.0".into()));
        let _ = writeln!(s, "   Gateway predefinito  . . . . . . : {}", d.gateway.map(|g| g.to_string()).unwrap_or("0.0.0.0".into()));
        if opt.as_deref() == Some("/all") {
            let _ = writeln!(s, "   Server DNS . . . . . . . . . . . : {}", d.dns_server.map(|g| g.to_string()).unwrap_or("0.0.0.0".into()));
        }
        if ifc.ip.is_some_and(|c| c.addr.is_link_local()) {
            s.push_str("   (Nessuna risposta dal server DHCP: indirizzo APIPA automatico)\n");
        }
    }
    s
}

fn ping(net: &mut Network, dev: DeviceId, args: &[&str]) -> String {
    let mut count = 4usize;
    let mut target = None;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-n" => {
                count = args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(4).clamp(1, 50);
                i += 1;
            }
            "-t" => count = 20,
            t => target = Some(t),
        }
        i += 1;
    }
    let Some(target) = target else { return "Uso: ping [-n N] <indirizzo IP o nome>".into() };
    let dst = match net.resolve_blocking(dev, target) {
        Ok(ip) => ip,
        Err(e) => return format!("Ping: impossibile trovare l'host {target} ({e})."),
    };
    let mut s = format!("\nEsecuzione di Ping {dst} con 32 byte di dati:\n");
    let (mut ok, mut lost) = (0, 0);
    for _ in 0..count {
        let idx = match net.ping_ttl(dev, dst, None) {
            Ok(i) => i,
            Err(e) => return e,
        };
        net.run_until_idle(REALTIME_MAX_TICKS);
        let rec = &net.pings[idx];
        match &rec.status {
            PingStatus::Success { ttl, .. } => {
                ok += 1;
                let _ = writeln!(s, "Risposta da {dst}: byte=32 durata<1ms TTL={ttl}");
            }
            PingStatus::Failed(r) => {
                lost += 1;
                match rec.reply_from {
                    Some(from) if r.contains("unreachable") => {
                        let _ = writeln!(s, "Risposta da {from}: host di destinazione non raggiungibile.");
                    }
                    Some(from) if r.contains("TTL") => {
                        let _ = writeln!(s, "Risposta da {from}: TTL scaduto durante il transito.");
                    }
                    _ if r.contains("nessuna rotta") || r.contains("unreachable") => {
                        let _ = writeln!(s, "Host di destinazione non raggiungibile.");
                    }
                    _ => {
                        let _ = writeln!(s, "Richiesta scaduta.");
                    }
                }
            }
            PingStatus::Pending => {
                lost += 1;
                let _ = writeln!(s, "Richiesta scaduta.");
            }
        }
    }
    let pct = lost * 100 / count;
    let _ = write!(
        s,
        "\nStatistiche Ping per {dst}:\n    Pacchetti: Trasmessi = {count}, Ricevuti = {ok}, Persi = {lost} ({pct}% persi),"
    );
    s
}

fn tracert(net: &mut Network, dev: DeviceId, target: Option<&str>) -> String {
    let Some(target) = target else { return "Uso: tracert <indirizzo IP o nome>".into() };
    let dst = match net.resolve_blocking(dev, target) {
        Ok(ip) => ip,
        Err(e) => return format!("Impossibile risolvere {target} ({e})."),
    };
    let hops = match net.traceroute(dev, dst, 30) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let mut s = format!("\nRilevazione instradamento verso {dst}\nsu un massimo di 30 punti di passaggio:\n\n");
    for h in &hops {
        match h.from {
            Some(ip) => writeln!(s, "  {:>2}    <1 ms    <1 ms    <1 ms    {ip}", h.ttl),
            None => writeln!(s, "  {:>2}     *        *        *       Richiesta scaduta.", h.ttl),
        }
        .ok();
    }
    s.push_str(if hops.last().is_some_and(|h| h.reached) { "\nRilevazione completata." } else { "\nDestinazione non raggiunta." });
    s
}

fn arp(net: &mut Network, dev: DeviceId, opt: Option<&str>) -> String {
    match opt {
        Some("-d") => {
            net.devices.get_mut(&dev).unwrap().state.arp_table.clear();
            "Cache ARP svuotata.".into()
        }
        Some("-a") | None => {
            let d = &net.devices[&dev];
            if d.state.arp_table.is_empty() {
                return "Nessuna voce ARP trovata.".into();
            }
            let mut s = format!("\n  {:<22}{:<20}{}\n", "Indirizzo Internet", "Indirizzo fisico", "Tipo");
            for (ip, mac) in &d.state.arp_table {
                let _ = writeln!(s, "  {:<22}{:<20}{}", ip.to_string(), mac.to_string(), "dinamico");
            }
            s
        }
        Some(o) => format!("Opzione non valida: {o}"),
    }
}

fn nslookup(net: &mut Network, dev: DeviceId, name: Option<&str>) -> String {
    let Some(name) = name else { return "Uso: nslookup <nome>".into() };
    let server = net.devices[&dev].dns_server;
    let idx = net.dns_resolve(dev, name);
    net.run_until_idle(REALTIME_MAX_TICKS);
    let srv = server.map(|s| s.to_string()).unwrap_or("(nessuno)".into());
    let mut s = format!("Server:  {srv}\n\n");
    match &net.dns_lookups[idx].status {
        DnsStatus::Resolved(ip) => {
            let _ = write!(s, "Nome:    {name}\nAddress: {ip}");
        }
        DnsStatus::NotFound => {
            let _ = write!(s, "*** {srv} non trova {name}: dominio inesistente");
        }
        DnsStatus::Failed(e) => {
            let _ = write!(s, "*** Richiesta al server non riuscita: {e}");
        }
        DnsStatus::Pending => s.push_str("*** Richiesta al server scaduta"),
    }
    s
}
