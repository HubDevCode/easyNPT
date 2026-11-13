//! NetPackeT core: motore di simulazione di rete, indipendente dall'interfaccia grafica.

pub mod addr;
pub mod cli;
pub mod device;
pub mod hostcmd;
pub mod packet;
pub mod sim;

pub use addr::{Ipv4Cidr, Mac};
pub use device::{Device, DeviceId, DeviceKind, Interface, LinkId, StaticRoute};
pub use sim::{Endpoint, EventKind, Link, Network, PingStatus};

#[cfg(test)]
mod tests;
