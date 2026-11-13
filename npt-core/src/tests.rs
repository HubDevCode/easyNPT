use crate::*;
use std::net::Ipv4Addr;

fn ip(s: &str) -> Ipv4Addr {
    s.parse().unwrap()
}

fn set_ip(net: &mut Network, dev: DeviceId, iface: usize, cidr: &str) {
    let d = net.device_mut(dev).unwrap();
    d.interfaces[iface].ip = Some(cidr.parse().unwrap());
    d.interfaces[iface].enabled = true;
}

fn last_ping(net: &Network) -> PingStatus {
    net.pings.last().unwrap().status.clone()
}

/// PC0 — Switch0 — PC1 sulla stessa subnet.
fn lan() -> (Network, DeviceId, DeviceId, DeviceId) {
    let mut net = Network::new();
    let pc0 = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let sw = net.add_device(DeviceKind::Switch, [100.0, 0.0]);
    let pc1 = net.add_device(DeviceKind::Pc, [200.0, 0.0]);
    net.connect_auto(pc0, sw).unwrap();
    net.connect_auto(pc1, sw).unwrap();
    set_ip(&mut net, pc0, 0, "192.168.1.10/24");
    set_ip(&mut net, pc1, 0, "192.168.1.11/24");
    (net, pc0, sw, pc1)
}

#[test]
fn names_are_unique() {
    let mut net = Network::new();
    let a = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let b = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    assert_eq!(net.device(a).unwrap().name, "PC0");
    assert_eq!(net.device(b).unwrap().name, "PC1");
}

#[test]
fn ping_same_lan_through_switch() {
    let (mut net, pc0, sw, pc1) = lan();
    net.ping(pc0, ip("192.168.1.11")).unwrap();
    net.run_until_idle(100);
    assert!(matches!(last_ping(&net), PingStatus::Success { ttl: 128, .. }), "{:?}", last_ping(&net));
    // Lo switch ha imparato entrambi i MAC.
    assert_eq!(net.device(sw).unwrap().state.mac_table.len(), 2);
    // Gli host hanno la cache ARP popolata.
    assert!(net.device(pc0).unwrap().state.arp_table.contains_key(&ip("192.168.1.11")));
    assert!(net.device(pc1).unwrap().state.arp_table.contains_key(&ip("192.168.1.10")));
}

#[test]
fn ping_unknown_host_times_out() {
    let (mut net, pc0, _, _) = lan();
    net.ping(pc0, ip("192.168.1.99")).unwrap();
    net.run_until_idle(100);
    assert!(matches!(last_ping(&net), PingStatus::Failed(_)));
    assert!(net.is_idle());
}

#[test]
fn ping_other_subnet_without_gateway_fails() {
    let (mut net, pc0, _, _) = lan();
    net.ping(pc0, ip("10.0.0.1")).unwrap();
    assert!(matches!(last_ping(&net), PingStatus::Failed(_)));
}

#[test]
fn hub_repeats_frames() {
    let mut net = Network::new();
    let a = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let h = net.add_device(DeviceKind::Hub, [0.0, 0.0]);
    let b = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let c = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    for d in [a, b, c] {
        net.connect_auto(d, h).unwrap();
    }
    set_ip(&mut net, a, 0, "10.0.0.1/8");
    set_ip(&mut net, b, 0, "10.0.0.2/8");
    set_ip(&mut net, c, 0, "10.0.0.3/8");
    net.ping(a, ip("10.0.0.2")).unwrap();
    net.run_until_idle(100);
    assert!(matches!(last_ping(&net), PingStatus::Success { .. }));
    // C riceve anche il traffico unicast (e lo scarta).
    assert!(net.events.iter().any(|e| e.device == c && e.kind == EventKind::Dropped));
}

/// PC0 (192.168.1.0/24) — R0 — PC1 (192.168.2.0/24)
fn routed() -> (Network, DeviceId, DeviceId, DeviceId) {
    let mut net = Network::new();
    let pc0 = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let r = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    let pc1 = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    net.connect(Endpoint { device: pc0, iface: 0 }, Endpoint { device: r, iface: 0 }).unwrap();
    net.connect(Endpoint { device: pc1, iface: 0 }, Endpoint { device: r, iface: 1 }).unwrap();
    set_ip(&mut net, pc0, 0, "192.168.1.10/24");
    set_ip(&mut net, pc1, 0, "192.168.2.10/24");
    set_ip(&mut net, r, 0, "192.168.1.1/24");
    set_ip(&mut net, r, 1, "192.168.2.1/24");
    net.device_mut(pc0).unwrap().gateway = Some(ip("192.168.1.1"));
    net.device_mut(pc1).unwrap().gateway = Some(ip("192.168.2.1"));
    (net, pc0, r, pc1)
}

#[test]
fn router_forwards_between_subnets() {
    let (mut net, pc0, _, _) = routed();
    net.ping(pc0, ip("192.168.2.10")).unwrap();
    net.run_until_idle(100);
    // Il TTL della risposta è decrementato di uno dal router.
    assert!(matches!(last_ping(&net), PingStatus::Success { ttl: 127, .. }), "{:?}", last_ping(&net));
}

#[test]
fn router_interfaces_start_shutdown() {
    let mut net = Network::new();
    let r = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    assert!(net.device(r).unwrap().interfaces.iter().all(|i| !i.enabled));
}

#[test]
fn shutdown_interface_breaks_path() {
    let (mut net, pc0, r, _) = routed();
    net.device_mut(r).unwrap().interfaces[1].enabled = false;
    net.ping(pc0, ip("192.168.2.10")).unwrap();
    net.run_until_idle(100);
    match last_ping(&net) {
        PingStatus::Failed(msg) => assert!(msg.contains("unreachable"), "{msg}"),
        s => panic!("atteso fallimento, ottenuto {s:?}"),
    }
}

#[test]
fn static_routes_across_two_routers() {
    // PC0 — R0 —(10.0.0.0/30)— R1 — PC1
    let mut net = Network::new();
    let pc0 = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let r0 = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    let r1 = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    let pc1 = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let ep = |d, i| Endpoint { device: d, iface: i };
    net.connect(ep(pc0, 0), ep(r0, 0)).unwrap();
    net.connect(ep(r0, 1), ep(r1, 1)).unwrap();
    net.connect(ep(r1, 0), ep(pc1, 0)).unwrap();
    set_ip(&mut net, pc0, 0, "192.168.1.10/24");
    set_ip(&mut net, r0, 0, "192.168.1.1/24");
    set_ip(&mut net, r0, 1, "10.0.0.1/30");
    set_ip(&mut net, r1, 1, "10.0.0.2/30");
    set_ip(&mut net, r1, 0, "192.168.2.1/24");
    set_ip(&mut net, pc1, 0, "192.168.2.10/24");
    net.device_mut(pc0).unwrap().gateway = Some(ip("192.168.1.1"));
    net.device_mut(pc1).unwrap().gateway = Some(ip("192.168.2.1"));

    // Senza rotte statiche R0 risponde "unreachable".
    net.ping(pc0, ip("192.168.2.10")).unwrap();
    net.run_until_idle(200);
    assert!(matches!(last_ping(&net), PingStatus::Failed(_)));

    net.device_mut(r0).unwrap().static_routes.push(StaticRoute {
        network: "192.168.2.0/24".parse().unwrap(),
        next_hop: ip("10.0.0.2"),
    });
    net.device_mut(r1).unwrap().static_routes.push(StaticRoute {
        network: "192.168.1.0/24".parse().unwrap(),
        next_hop: ip("10.0.0.1"),
    });
    net.ping(pc0, ip("192.168.2.10")).unwrap();
    net.run_until_idle(200);
    assert!(matches!(last_ping(&net), PingStatus::Success { ttl: 126, .. }), "{:?}", last_ping(&net));
}

#[test]
fn routing_loop_triggers_ttl_expired() {
    let (mut net, pc0, r, _) = routed();
    // Una seconda rete con rotta che punta indietro verso PC0... simuliamo un loop
    // con due router che si rimandano il traffico.
    let r1 = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    net.connect(Endpoint { device: r, iface: 2 }, Endpoint { device: r1, iface: 0 }).unwrap();
    set_ip(&mut net, r, 2, "10.0.0.1/30");
    set_ip(&mut net, r1, 0, "10.0.0.2/30");
    let dst: Ipv4Cidr = "172.16.0.0/16".parse().unwrap();
    net.device_mut(r).unwrap().static_routes.push(StaticRoute { network: dst, next_hop: ip("10.0.0.2") });
    net.device_mut(r1).unwrap().static_routes.push(StaticRoute { network: dst, next_hop: ip("10.0.0.1") });
    // Il TTL scade su R1: deve sapere come tornare a PC0 per inviare l'errore ICMP.
    net.device_mut(r1).unwrap().static_routes.push(StaticRoute {
        network: "192.168.1.0/24".parse().unwrap(),
        next_hop: ip("10.0.0.1"),
    });
    net.ping(pc0, ip("172.16.0.5")).unwrap();
    net.run_until_idle(2000);
    match last_ping(&net) {
        PingStatus::Failed(msg) => assert!(msg.contains("TTL expired"), "{msg}"),
        s => panic!("atteso TTL expired, ottenuto {s:?}"),
    }
}

#[test]
fn save_and_load_roundtrip() {
    let (net, ..) = routed();
    let json = net.to_json().unwrap();
    let mut loaded = Network::from_json(&json).unwrap();
    assert_eq!(loaded.devices.len(), 3);
    assert_eq!(loaded.links.len(), 2);
    let pc0 = loaded.device_by_name("PC0").unwrap();
    loaded.ping(pc0, ip("192.168.2.10")).unwrap();
    loaded.run_until_idle(100);
    assert!(matches!(last_ping(&loaded), PingStatus::Success { .. }));
    // Aggiungere un dispositivo dopo il caricamento non deve riusare id o MAC.
    let new = loaded.add_device(DeviceKind::Pc, [0.0, 0.0]);
    assert!(!net.devices.contains_key(&new));
}

#[test]
fn disconnect_clears_mac_table() {
    let (mut net, pc0, sw, _) = lan();
    net.ping(pc0, ip("192.168.1.11")).unwrap();
    net.run_until_idle(100);
    let link = net.device(pc0).unwrap().interfaces[0].link.unwrap();
    net.disconnect(link);
    assert_eq!(net.device(sw).unwrap().state.mac_table.len(), 1);
}

// ----------------------------------------------------------------------
// VLAN, trunk, router-on-a-stick, SVI
// ----------------------------------------------------------------------

use crate::device::{DhcpPool, PortMode};

/// Switch con PC0 e PC1 in VLAN 10, PC2 in VLAN 20.
fn vlan_lab() -> (Network, DeviceId, [DeviceId; 3]) {
    let mut net = Network::new();
    let sw = net.add_device(DeviceKind::Switch, [0.0, 0.0]);
    let pcs = [0, 1, 2].map(|_| net.add_device(DeviceKind::Pc, [0.0, 0.0]));
    for (i, pc) in pcs.iter().enumerate() {
        net.connect(Endpoint { device: *pc, iface: 0 }, Endpoint { device: sw, iface: i }).unwrap();
    }
    let d = net.device_mut(sw).unwrap();
    d.vlans.insert(10, "STUDENTI".into());
    d.vlans.insert(20, "DOCENTI".into());
    d.interfaces[0].switchport.access_vlan = 10;
    d.interfaces[1].switchport.access_vlan = 10;
    d.interfaces[2].switchport.access_vlan = 20;
    set_ip(&mut net, pcs[0], 0, "192.168.10.10/24");
    set_ip(&mut net, pcs[1], 0, "192.168.10.11/24");
    set_ip(&mut net, pcs[2], 0, "192.168.10.12/24");
    (net, sw, pcs)
}

#[test]
fn vlans_isolate_broadcast_domains() {
    let (mut net, _, pcs) = vlan_lab();
    net.ping(pcs[0], ip("192.168.10.11")).unwrap();
    net.run_until_idle(100);
    assert!(matches!(last_ping(&net), PingStatus::Success { .. }));
    // Stessa subnet ma VLAN diversa: non raggiungibile.
    net.ping(pcs[0], ip("192.168.10.12")).unwrap();
    net.run_until_idle(100);
    assert!(matches!(last_ping(&net), PingStatus::Failed(_)));
    // PC2 non ha mai visto l'ARP di PC0.
    assert!(!net.events.iter().any(|e| e.device == pcs[2]));
}

#[test]
fn access_port_in_missing_vlan_drops() {
    let (mut net, sw, pcs) = vlan_lab();
    net.device_mut(sw).unwrap().vlans.remove(&10);
    net.ping(pcs[0], ip("192.168.10.11")).unwrap();
    net.run_until_idle(100);
    assert!(matches!(last_ping(&net), PingStatus::Failed(_)));
}

/// Router-on-a-stick: VLAN 10 e 20 instradate dalla sottointerfacce di R0.
fn router_on_a_stick() -> (Network, DeviceId, DeviceId, [DeviceId; 2]) {
    let mut net = Network::new();
    let r = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    let sw = net.add_device(DeviceKind::Switch, [0.0, 0.0]);
    let a = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let b = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    net.connect(Endpoint { device: r, iface: 0 }, Endpoint { device: sw, iface: 24 }).unwrap();
    net.connect(Endpoint { device: a, iface: 0 }, Endpoint { device: sw, iface: 0 }).unwrap();
    net.connect(Endpoint { device: b, iface: 0 }, Endpoint { device: sw, iface: 1 }).unwrap();
    {
        let s = net.device_mut(sw).unwrap();
        s.vlans.insert(10, "A".into());
        s.vlans.insert(20, "B".into());
        s.interfaces[0].switchport.access_vlan = 10;
        s.interfaces[1].switchport.access_vlan = 20;
        s.interfaces[24].switchport.mode = PortMode::Trunk;
    }
    {
        let rd = net.device_mut(r).unwrap();
        rd.interfaces[0].enabled = true;
        for (n, v, ipa) in [(10u32, 10u16, "192.168.10.1/24"), (20, 20, "192.168.20.1/24")] {
            let i = rd.ensure_subinterface(0, n);
            rd.interfaces[i].kind = crate::device::IfaceKind::Sub { parent: 0, vlan: v };
            rd.interfaces[i].ip = Some(ipa.parse().unwrap());
        }
    }
    set_ip(&mut net, a, 0, "192.168.10.10/24");
    set_ip(&mut net, b, 0, "192.168.20.10/24");
    net.device_mut(a).unwrap().gateway = Some(ip("192.168.10.1"));
    net.device_mut(b).unwrap().gateway = Some(ip("192.168.20.1"));
    (net, r, sw, [a, b])
}

#[test]
fn inter_vlan_routing_router_on_a_stick() {
    let (mut net, _, _, [a, _]) = router_on_a_stick();
    net.ping(a, ip("192.168.20.10")).unwrap();
    net.run_until_idle(200);
    assert!(matches!(last_ping(&net), PingStatus::Success { ttl: 127, .. }), "{:?}", last_ping(&net));
    // Sul trunk i frame viaggiano con tag 802.1Q.
    assert!(net.events.iter().any(|e| e.frame.as_ref().is_some_and(|f| f.vlan == Some(20))));
}

#[test]
fn trunk_allowed_list_blocks_vlan() {
    let (mut net, _, sw, [a, _]) = router_on_a_stick();
    net.device_mut(sw).unwrap().interfaces[24].switchport.allowed = Some(vec![10]);
    net.ping(a, ip("192.168.20.10")).unwrap();
    net.run_until_idle(200);
    assert!(matches!(last_ping(&net), PingStatus::Failed(_)));
}

#[test]
fn switch_svi_answers_ping() {
    let (mut net, _, sw, [a, _]) = router_on_a_stick();
    {
        let s = net.device_mut(sw).unwrap();
        let mac = s.interfaces[0].mac; // qualsiasi MAC diverso dagli host
        let svi = s.ensure_svi(10, Mac([0x02, 0, 0, 0, 0x99, mac.0[5]]));
        s.interfaces[svi].ip = Some("192.168.10.2/24".parse().unwrap());
        s.interfaces[svi].enabled = true;
    }
    net.ping(a, ip("192.168.10.2")).unwrap();
    net.run_until_idle(200);
    assert!(matches!(last_ping(&net), PingStatus::Success { ttl: 255, .. }), "{:?}", last_ping(&net));
}

// ----------------------------------------------------------------------
// DHCP, DNS, traceroute
// ----------------------------------------------------------------------

#[test]
fn dhcp_from_server_on_same_lan() {
    let (mut net, pc0, _, pc1) = lan();
    // PC1 diventa un server DHCP.
    let srv = pc1;
    {
        let s = net.device_mut(srv).unwrap();
        s.services.dhcp = true;
        let mut pool = DhcpPool::new("serverPool");
        pool.network = Some("192.168.1.0/24".parse().unwrap());
        pool.default_router = Some(ip("192.168.1.1"));
        pool.dns = Some(ip("192.168.1.11"));
        pool.start = Some(ip("192.168.1.100"));
        s.dhcp_pools.push(pool);
    }
    net.dhcp_request(pc0, 0);
    net.run_until_idle(100);
    let d = net.device(pc0).unwrap();
    assert_eq!(d.interfaces[0].ip, Some("192.168.1.100/24".parse().unwrap()));
    assert_eq!(d.gateway, Some(ip("192.168.1.1")));
    assert_eq!(d.dns_server, Some(ip("192.168.1.11")));
    assert_eq!(net.device(srv).unwrap().dhcp_leases.len(), 1);
}

#[test]
fn dhcp_without_server_gives_apipa() {
    let (mut net, pc0, _, _) = lan();
    net.dhcp_request(pc0, 0);
    net.run_until_idle(100);
    let ip0 = net.device(pc0).unwrap().interfaces[0].ip.unwrap();
    assert_eq!(ip0.addr.octets()[..2], [169, 254]);
}

#[test]
fn dhcp_relay_with_helper_address() {
    // PC0 (DHCP) — R0 — Server (192.168.2.100) con pool per 192.168.1.0/24
    let (mut net, pc0, r, srv) = routed();
    net.device_mut(r).unwrap().interfaces[0].helper = Some(ip("192.168.2.10"));
    {
        let s = net.device_mut(srv).unwrap();
        s.services.dhcp = true;
        let mut pool = DhcpPool::new("LAN1");
        pool.network = Some("192.168.1.0/24".parse().unwrap());
        pool.default_router = Some(ip("192.168.1.1"));
        s.dhcp_pools.push(pool);
    }
    net.dhcp_request(pc0, 0);
    net.run_until_idle(200);
    let d = net.device(pc0).unwrap();
    assert_eq!(d.interfaces[0].ip, Some("192.168.1.2/24".parse().unwrap()), "{:?}", d.state.dhcp);
    // E ora la connettività funziona.
    net.ping(pc0, ip("192.168.2.10")).unwrap();
    net.run_until_idle(200);
    assert!(matches!(last_ping(&net), PingStatus::Success { .. }));
}

#[test]
fn router_dhcp_pool_with_exclusions() {
    let (mut net, pc0, r, _) = routed();
    {
        let rd = net.device_mut(r).unwrap();
        let mut pool = DhcpPool::new("LAN1");
        pool.network = Some("192.168.1.0/24".parse().unwrap());
        pool.default_router = Some(ip("192.168.1.1"));
        rd.dhcp_pools.push(pool);
        rd.dhcp_excluded.push((ip("192.168.1.1"), ip("192.168.1.9")));
    }
    net.dhcp_request(pc0, 0);
    net.run_until_idle(200);
    assert_eq!(net.device(pc0).unwrap().interfaces[0].ip, Some("192.168.1.10/24".parse().unwrap()));
}

#[test]
fn dns_resolution_and_ping_by_name() {
    let (mut net, pc0, _, pc1) = routed();
    {
        let s = net.device_mut(pc1).unwrap();
        s.services.dns = true;
        s.dns_records.push(("www.scuola.it".into(), ip("192.168.2.10")));
    }
    net.device_mut(pc0).unwrap().dns_server = Some(ip("192.168.2.10"));
    assert_eq!(net.resolve_blocking(pc0, "WWW.scuola.it"), Ok(ip("192.168.2.10")));
    assert!(net.resolve_blocking(pc0, "nonesiste.it").is_err());
}

#[test]
fn traceroute_lists_each_hop() {
    let (mut net, pc0, _, _) = routed();
    let hops = net.traceroute(pc0, ip("192.168.2.10"), 30).unwrap();
    assert_eq!(hops.len(), 2);
    assert_eq!(hops[0].from, Some(ip("192.168.1.1")));
    assert_eq!(hops[1].from, Some(ip("192.168.2.10")));
    assert!(hops[1].reached);
}

// ----------------------------------------------------------------------
// CLI in stile IOS e prompt dei comandi
// ----------------------------------------------------------------------

use crate::cli::{CliMode, CliSession};

fn cli(net: &mut Network, dev: DeviceId, s: &mut CliSession, lines: &str) -> String {
    let mut out = String::new();
    for l in lines.lines() {
        out.push_str(&s.execute(net, dev, l));
        out.push('\n');
    }
    out
}

#[test]
fn cli_configures_router_with_abbreviations() {
    let mut net = Network::new();
    let r = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    let pc = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    net.connect(Endpoint { device: pc, iface: 0 }, Endpoint { device: r, iface: 0 }).unwrap();
    set_ip(&mut net, pc, 0, "192.168.1.10/24");
    net.device_mut(pc).unwrap().gateway = Some(ip("192.168.1.1"));

    let mut s = CliSession::new();
    assert_eq!(s.prompt(&net, r), "Router0>");
    let out = cli(&mut net, r, &mut s, "en\nconf t\nhostname R1\nint g0/0\nip add 192.168.1.1 255.255.255.0\nno sh\nend");
    assert!(!out.contains('%'), "{out}");
    assert_eq!(s.mode, CliMode::Enable);
    assert_eq!(s.prompt(&net, r), "R1#");
    let d = net.device(r).unwrap();
    assert_eq!(d.interfaces[0].ip, Some("192.168.1.1/24".parse().unwrap()));
    assert!(d.interfaces[0].enabled);

    let brief = s.execute(&mut net, r, "sh ip int br");
    assert!(brief.contains("GigabitEthernet0/0") && brief.contains("192.168.1.1") && brief.contains("up"), "{brief}");
    let route = s.execute(&mut net, r, "show ip route");
    assert!(route.contains("C    192.168.1.0/24 is directly connected"), "{route}");
    let ping = s.execute(&mut net, r, "ping 192.168.1.10");
    assert!(ping.contains("!!!!!") && ping.contains("100 percent"), "{ping}");
    let run = s.execute(&mut net, r, "show run");
    assert!(run.contains("hostname R1") && run.contains("ip address 192.168.1.1 255.255.255.0"), "{run}");
}

#[test]
fn cli_errors_and_help() {
    let mut net = Network::new();
    let r = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    let mut s = CliSession::new();
    assert!(s.execute(&mut net, r, "configure terminal").contains("Invalid"));
    assert!(s.execute(&mut net, r, "?").contains("enable"));
    s.execute(&mut net, r, "enable");
    assert!(s.execute(&mut net, r, "co").contains("Ambiguous"));
    s.execute(&mut net, r, "conf t");
    assert!(s.execute(&mut net, r, "int g0/0").is_empty());
    assert!(s.execute(&mut net, r, "ip address 10.0.0.0 255.0.0.0").contains('%')); // indirizzo di rete
    // Comando globale dentro config-if: torna a config.
    s.execute(&mut net, r, "hostname X");
    assert_eq!(net.device(r).unwrap().name, "X");
    // do in configurazione
    assert!(s.execute(&mut net, r, "do show ip int brief").contains("Interface"));
}

#[test]
fn cli_enable_secret_and_banner() {
    let mut net = Network::new();
    let r = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    let mut s = CliSession::new();
    cli(&mut net, r, &mut s, "en\nconf t\nenable secret cisco\nbanner motd #Accesso riservato#\nend\ndisable");
    assert!(s.greeting(&net, r).contains("Accesso riservato"));
    s.execute(&mut net, r, "enable");
    assert_eq!(s.prompt(&net, r), "Password: ");
    s.execute(&mut net, r, "sbagliata");
    assert_eq!(s.mode, CliMode::User);
    s.execute(&mut net, r, "enable");
    s.execute(&mut net, r, "cisco");
    assert_eq!(s.mode, CliMode::Enable);
}

#[test]
fn cli_builds_router_on_a_stick_and_vlans() {
    let mut net = Network::new();
    let r = net.add_device(DeviceKind::Router, [0.0, 0.0]);
    let sw = net.add_device(DeviceKind::Switch, [0.0, 0.0]);
    let a = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    let b = net.add_device(DeviceKind::Pc, [0.0, 0.0]);
    net.connect(Endpoint { device: r, iface: 0 }, Endpoint { device: sw, iface: 24 }).unwrap();
    net.connect(Endpoint { device: a, iface: 0 }, Endpoint { device: sw, iface: 0 }).unwrap();
    net.connect(Endpoint { device: b, iface: 0 }, Endpoint { device: sw, iface: 1 }).unwrap();
    set_ip(&mut net, a, 0, "192.168.10.10/24");
    set_ip(&mut net, b, 0, "192.168.20.10/24");
    net.device_mut(a).unwrap().gateway = Some(ip("192.168.10.1"));
    net.device_mut(b).unwrap().gateway = Some(ip("192.168.20.1"));

    let mut s = CliSession::new();
    let out = cli(
        &mut net,
        sw,
        &mut s,
        "en\nconf t\nvlan 10\nname STUDENTI\nvlan 20\nname DOCENTI\nexit\nint fa0/1\nswitchport mode access\nswitchport access vlan 10\n\
         int fa0/2\nsw acc vl 20\nint g0/1\nswitchport mode trunk\nend",
    );
    assert!(!out.contains("% Invalid"), "{out}");
    let vl = s.execute(&mut net, sw, "show vlan brief");
    assert!(vl.contains("STUDENTI") && vl.contains("Fa0/1"), "{vl}");
    assert!(s.execute(&mut net, sw, "show interfaces trunk").contains("Gi0/1"));

    let mut rs = CliSession::new();
    let out = cli(
        &mut net,
        r,
        &mut rs,
        "en\nconf t\nint g0/0\nno shut\nint g0/0.10\nencapsulation dot1Q 10\nip address 192.168.10.1 255.255.255.0\n\
         int g0/0.20\nencapsulation dot1q 20\nip address 192.168.20.1 255.255.255.0\nend",
    );
    assert!(!out.contains('%'), "{out}");
    assert_eq!(rs.prompt(&net, r), "Router0#");
    net.ping(a, ip("192.168.20.10")).unwrap();
    net.run_until_idle(300);
    assert!(matches!(last_ping(&net), PingStatus::Success { .. }), "{:?}", last_ping(&net));
}

#[test]
fn cli_interface_range_and_dhcp_pool() {
    let mut net = Network::new();
    let sw = net.add_device(DeviceKind::Switch, [0.0, 0.0]);
    let mut s = CliSession::new();
    cli(&mut net, sw, &mut s, "en\nconf t\nint range fa0/1 - 5, fa0/10\nswitchport access vlan 30\nshutdown\nend");
    let d = net.device(sw).unwrap();
    assert!((0..5).chain([9]).all(|i| d.interfaces[i].switchport.access_vlan == 30 && !d.interfaces[i].enabled));
    assert!(d.interfaces[5].enabled);
    assert!(d.vlans.contains_key(&30));

    let (mut net, pc0, r, _) = routed();
    let mut rs = CliSession::new();
    let out = cli(
        &mut net,
        r,
        &mut rs,
        "en\nconf t\nip dhcp excluded-address 192.168.1.1 192.168.1.20\nip dhcp pool LAN1\nnetwork 192.168.1.0 255.255.255.0\n\
         default-router 192.168.1.1\ndns-server 8.8.8.8\nend",
    );
    assert!(!out.contains('%'), "{out}");
    net.dhcp_request(pc0, 0);
    net.run_until_idle(200);
    assert_eq!(net.device(pc0).unwrap().interfaces[0].ip, Some("192.168.1.21/24".parse().unwrap()));
    assert!(rs.execute(&mut net, r, "show ip dhcp binding").contains("192.168.1.21"));
}

#[test]
fn cli_static_route_and_traceroute() {
    let (mut net, pc0, r, _) = routed();
    let mut s = CliSession::new();
    cli(&mut net, r, &mut s, "en\nconf t\nip route 10.0.0.0 255.0.0.0 192.168.2.10\nend");
    assert!(s.execute(&mut net, r, "show ip route").contains("S    10.0.0.0/8 [1/0] via 192.168.2.10"));
    cli(&mut net, r, &mut s, "conf t\nno ip route 10.0.0.0 255.0.0.0 192.168.2.10\nend");
    assert!(net.device(r).unwrap().static_routes.is_empty());
    let out = crate::hostcmd::run_host_command(&mut net, pc0, "tracert 192.168.2.10");
    assert!(out.contains("192.168.1.1") && out.contains("Rilevazione completata"), "{out}");
}

#[test]
fn host_prompt_commands() {
    let (mut net, pc0, ..) = routed();
    use crate::hostcmd::run_host_command as run;
    let out = run(&mut net, pc0, "ping 192.168.2.10");
    assert!(out.contains("Ricevuti = 4"), "{out}");
    assert!(run(&mut net, pc0, "arp -a").contains("192.168.1.1"));
    assert!(run(&mut net, pc0, "ipconfig").contains("192.168.1.10"));
    assert!(run(&mut net, pc0, "ipconfig /all").contains("MAC"));
    let out = run(&mut net, pc0, "ping 10.9.9.9");
    assert!(out.contains("non raggiungibile") && out.contains("Persi = 4"), "{out}");
    assert!(run(&mut net, pc0, "pippo").contains("non è riconosciuto"));
}
