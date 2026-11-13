//! NetPackeT core: motore di simulazione di rete, indipendente dall'interfaccia grafica.

pub mod addr;
pub mod device;
pub mod packet;
pub mod sim;

pub use addr::{Ipv4Cidr, Mac};
pub use device::{Device, DeviceId, DeviceKind, Interface, LinkId, StaticRoute};
pub use sim::{Endpoint, EventKind, Link, Network, PingStatus};

#[cfg(test)]
mod tests;
