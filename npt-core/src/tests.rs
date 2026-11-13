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
