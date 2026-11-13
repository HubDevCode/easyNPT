# NetPackeT — *NPT easy*

Simulatore di reti didattico, nativo per Linux, scritto in Rust.
Progetto originale: il comportamento della rete è implementato a partire dagli
standard pubblici (IEEE 802.3, RFC 791/792/826, …).

## Avvio

```bash
cargo run --release -p npt-app                                # progetto vuoto
cargo run --release -p npt-app -- esempi/due-lan-un-router.npt  # apre un esempio
cargo test -p npt-core                                         # test del motore
```

L'eseguibile finale è `target/release/npt-app` (un solo file, nessuna dipendenza
da installare oltre alle librerie grafiche di sistema).

## Struttura

| Cartella   | Contenuto |
|------------|-----------|
| `npt-core` | Motore di simulazione, senza interfaccia grafica: dispositivi, cavi, PDU, protocolli, salvataggio. Testato con `cargo test`. |
| `npt-app`  | Interfaccia grafica (egui/eframe): area di lavoro, palette, proprietà, ping, modalità simulazione. |
| `esempi`   | Reti di esempio in formato `.npt` (JSON). |

## Cosa funziona (fase 1)

- Dispositivi: PC, Laptop, Server, Hub, Switch (24 FastEthernet + 2 Gigabit), Router (3 Gigabit)
- Posa cavi con scelta delle porte, cavo dritto/incrociato automatico, luci di stato verde/rossa
- Configurazione IP/subnet mask/gateway con validazione (IP duplicati, rete/broadcast, subnet sovrapposte)
- Interfacce dei router spente di default (`no shutdown` dal pannello)
- Ethernet, ARP (cache e richieste in attesa), switch con apprendimento MAC e flooding, hub
- IPv4 con TTL, inoltro dei router, rotte connesse e statiche, default gateway
- ICMP Echo, Destination Unreachable, Time Exceeded (loop di routing)
- Modalità **tempo reale** e **simulazione** (pacchetti animati, passo-passo, log eventi, ispettore PDU per livelli)
- Tabelle di routing, ARP e MAC visibili nel pannello proprietà
- Salvataggio/apertura file `.npt`

## Roadmap

1. ~~Base: dispositivi, cavi, IP, ARP, ICMP, switch, hub, router con rotte statiche~~
2. CLI dei dispositivi (modalità utente/privilegiata/config, `show`, `interface`, `ip address`, `ip route`…)
3. VLAN, trunk 802.1Q, router-on-a-stick, switch L3
4. Routing dinamico: RIP v2, OSPF area singola
5. Servizi: DHCP, DNS, HTTP, NAT/PAT, ACL standard ed estese
6. STP, EtherChannel, wireless, IPv6
7. Note sul canvas, zoom, attività con verifica automatica per il docente
8. Pacchetti per le distribuzioni (AppImage, Flatpak, .deb) e build Windows
