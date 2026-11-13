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

## Cosa funziona

**Interfaccia**
- Palette a sinistra (strumenti + dispositivi), pannello laterale destro con schede *Dettagli*, *Simulazione*, *PDU*
- Doppio clic su un dispositivo: finestra con *Config* (moduli grafici + "comandi IOS equivalenti"),
  *CLI* (router/switch), *Desktop* (Configurazione IP, Prompt dei comandi), *Servizi* (server: DHCP, DNS)
- Strumento *PDU semplice* (ping sorgente → destinazione) con elenco esiti, note sul canvas, zoom (Ctrl+rotella)
- Modalità tempo reale e simulazione (pacchetti animati colorati per protocollo, log eventi, ispettore OSI)

**Rete**
- Dispositivi: PC, Laptop, Server, Hub, Switch (24 Fa + 2 Gi, SVI `Vlan1`), Router (3 Gi)
- Ethernet, ARP, switch con tabella MAC per VLAN, hub
- VLAN, porte access/trunk 802.1Q (VLAN nativa, allowed list), router-on-a-stick con sottointerfacce, SVI
- IPv4, rotte connesse e statiche, default gateway, TTL
- ICMP (echo, unreachable, TTL exceeded), traceroute
- DHCP (server su Server o Router, pool, indirizzi esclusi, relay con `ip helper-address`, APIPA se nessuno risponde)
- DNS (server con record A, `nslookup`, ping per nome)

**CLI in stile IOS** (abbreviazioni, `?`, `do`, `no`)
`enable`, `configure terminal`, `hostname`, `interface` / `interface range`, `ip address` / `ip address dhcp`,
`shutdown`, `description`, `switchport mode|access vlan|trunk native|allowed vlan`, `encapsulation dot1Q`,
`vlan` / `name`, `ip route`, `ip default-gateway`, `ip dhcp pool` / `network` / `default-router` / `dns-server`,
`ip dhcp excluded-address`, `ip helper-address`, `enable secret`, `banner motd`, `ping`, `traceroute`,
`show running-config | ip interface brief | ip route | vlan brief | mac address-table | interfaces [trunk] | arp | ip dhcp binding | cdp neighbors | version`,
`copy run start`, `write`, `clear`, `reload`.

**Prompt dei PC**: `ipconfig [/all|/renew|/release]`, `ping [-n N]`, `tracert`, `arp -a|-d`, `nslookup`, `cls`.

## Roadmap

1. ~~Base: dispositivi, cavi, IP, ARP, ICMP, switch, hub, router con rotte statiche~~
2. ~~CLI IOS, VLAN, trunk, router-on-a-stick, DHCP (+relay), DNS, traceroute, finestre dispositivo~~
3. Routing dinamico: RIP v2, OSPF area singola
4. ACL standard ed estese, NAT/PAT
5. Switch multilayer (L3), STP, EtherChannel, port-security
6. HTTP + browser web nel Desktop, e-mail, FTP
7. Wireless (access point, laptop Wi-Fi), IPv6, cavi seriali/fibra
8. Attività con verifica automatica per il docente
9. Pacchetti per le distribuzioni (AppImage, Flatpak, .deb) e build Windows

Per i test dell'interfaccia: `NPT_APRI=Router0@cli,PC0@prompt` apre le finestre indicate all'avvio
(schede: `config`, `cli`, `desktop`, `ip`, `prompt`, `servizi`).
