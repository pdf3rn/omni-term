//! Conexiones de terminal: SSH, Telnet, puerto serie, terminal local y lo que las rodea (túneles, hosts conocidos).

pub mod known_hosts;
pub mod local;
pub mod serial;
pub mod socks;
pub mod ssh;
pub mod sshconfig;
pub mod telnet;
