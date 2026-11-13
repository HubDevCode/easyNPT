//! Genera `esempi/due-lan-un-router.npt`: due LAN collegate da un router.
//! Uso: cargo run -p npt-core --example crea_esempio

use npt_core::{DeviceKind, Endpoint, Network};

fn main() {
    let mut net = Network::new();
    let r0 = net.add_device(DeviceKind::Router, [480.0, 120.0]);
    let sw0 = net.add_device(DeviceKind::Switch, [260.0, 280.0]);
    let sw1 = net.add_device(DeviceKind::Switch, [700.0, 280.0]);
    let pcs: Vec<_> = [[140.0, 450.0], [300.0, 450.0], [620.0, 450.0]]
        .into_iter()
        .map(|p| net.add_device(DeviceKind::Pc, p))
        .collect();
    let srv = net.add_device(DeviceKind::Server, [800.0, 450.0]);

    let ep = |device, iface| Endpoint { device, iface };
    net.connect(ep(r0, 0), ep(sw0, 24)).unwrap();
    net.connect(ep(r0, 1), ep(sw1, 24)).unwrap();
    net.connect(ep(pcs[0], 0), ep(sw0, 0)).unwrap();
    net.connect(ep(pcs[1], 0), ep(sw0, 1)).unwrap();
    net.connect(ep(pcs[2], 0), ep(sw1, 0)).unwrap();
    net.connect(ep(srv, 0), ep(sw1, 1)).unwrap();

    let cfg = |net: &mut Network, d, i: usize, ip: &str| {
        let dev = net.device_mut(d).unwrap();
        dev.interfaces[i].ip = Some(ip.parse().unwrap());
        dev.interfaces[i].enabled = true;
    };
    cfg(&mut net, r0, 0, "192.168.1.1/24");
    cfg(&mut net, r0, 1, "192.168.2.1/24");
    for (d, ip, gw) in [
        (pcs[0], "192.168.1.10/24", "192.168.1.1"),
        (pcs[1], "192.168.1.11/24", "192.168.1.1"),
        (pcs[2], "192.168.2.10/24", "192.168.2.1"),
        (srv, "192.168.2.100/24", "192.168.2.1"),
    ] {
        cfg(&mut net, d, 0, ip);
        net.device_mut(d).unwrap().gateway = Some(gw.parse().unwrap());
    }

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../esempi/due-lan-un-router.npt");
    std::fs::write(path, net.to_json().unwrap()).unwrap();
    println!("Scritto {path}");
}
