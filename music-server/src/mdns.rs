//! Announce the server on the LAN (mDNS / DNS-SD) so the Android app finds it without typing an IP.
//!
//! Service type `_music-server._tcp`, instance name "Music Server" (override with `MUSIC_MDNS_NAME`),
//! TXT records `version`, `api`, `path`. On Android: `NsdManager.discoverServices("_music-server._tcp", ...)`.
//!
//! Only the interface that carries the address of `MUSIC_PUBLIC_URL` is used: the Pi also has Docker
//! bridges (172.17/172.18) that must never be advertised. The host name is deliberately different from the
//! machine's own `.local` name, which Avahi already owns on Raspberry Pi OS.

use anyhow::{anyhow, Result};
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use std::net::IpAddr;

pub const SERVICE_TYPE: &str = "_music-server._tcp.local.";

pub struct Advert {
    daemon: ServiceDaemon,
    fullname: String,
}

/// `http://192.168.1.50:8765/` -> 192.168.1.50
pub fn ip_from_url(url: &str) -> Result<IpAddr> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    host.parse().map_err(|_| anyhow!("MUSIC_PUBLIC_URL host '{host}' is not an IP address: cannot advertise over mDNS"))
}

fn machine_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "server".into())
}

/// DNS labels: lowercase letters, digits and hyphens only.
fn label(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect::<String>().trim_matches('-').to_string()
}

pub fn start(public_url: &str, port: u16, instance: &str) -> Result<Advert> {
    let ip = ip_from_url(public_url)?;
    let host = format!("{}-music.local.", label(&machine_name()));
    let daemon = ServiceDaemon::new()?;
    daemon.disable_interface(IfKind::All)?;
    daemon.enable_interface(IfKind::Addr(ip))?;
    let props = [("version", env!("CARGO_PKG_VERSION")), ("api", "1"), ("path", "/")];
    let info = ServiceInfo::new(SERVICE_TYPE, instance, &host, ip, port, &props[..])?;
    let fullname = info.get_fullname().to_string();
    daemon.register(info)?;
    tracing::info!("mDNS: announcing '{instance}' as {SERVICE_TYPE} on {ip}:{port} (host {host})");
    Ok(Advert { daemon, fullname })
}

impl Advert {
    /// Send the goodbye packets so clients drop the service at once.
    pub fn stop(self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
        }
        let _ = self.daemon.shutdown();
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ip_from_public_url() {
        assert_eq!(ip_from_url("http://192.168.1.50:8765").unwrap().to_string(), "192.168.1.50");
        assert_eq!(ip_from_url("http://10.0.0.2:80/").unwrap().to_string(), "10.0.0.2");
        assert!(ip_from_url("http://raspberrypi.local:8765").is_err());
    }

    #[test]
    fn dns_label() {
        assert_eq!(label("raspberrypi"), "raspberrypi");
        assert_eq!(label("My Mac_Book"), "my-mac-book");
    }
}
