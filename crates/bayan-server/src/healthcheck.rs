//! The `healthcheck` subcommand used by the container's health check.
//!
//! The final container image has no shell or `curl`, so the server binary checks itself: it sends `GET /healthz` to the configured listen port on the loopback interface and exits with status 0 only on `200 OK`. It uses the standard library alone, with short timeouts.

use std::io::{Read as _, Write as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// How long each step (connect, write, read) may take.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Why the health check failed.
#[derive(Debug, thiserror::Error)]
pub enum HealthcheckError {
    /// The server could not be reached.
    #[error("cannot reach the server: {0}")]
    Io(std::io::ErrorKind),
    /// The server answered with something other than `200 OK`.
    #[error("unexpected response: {0}")]
    Status(String),
}

/// Checks `GET /healthz` on the loopback address for `listen`.
///
/// # Errors
///
/// Returns an error if the server cannot be reached or does not answer `200 OK`.
pub fn check(listen: SocketAddr) -> Result<(), HealthcheckError> {
    let target = SocketAddr::new(loopback_for(listen.ip()), listen.port());
    let io = |error: std::io::Error| HealthcheckError::Io(error.kind());
    let mut stream = TcpStream::connect_timeout(&target, TIMEOUT).map_err(io)?;
    stream.set_read_timeout(Some(TIMEOUT)).map_err(io)?;
    stream.set_write_timeout(Some(TIMEOUT)).map_err(io)?;
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(io)?;
    let mut head = [0_u8; 64];
    let mut len = 0;
    while len < head.len() {
        match stream.read(&mut head[len..]).map_err(io)? {
            0 => break,
            n => len += n,
        }
    }
    let status_line = String::from_utf8_lossy(&head[..len]);
    let status_line = status_line.lines().next().unwrap_or_default();
    if status_line.starts_with("HTTP/1.1 200 ") {
        Ok(())
    } else {
        Err(HealthcheckError::Status(
            status_line.chars().take(40).collect(),
        ))
    }
}

/// A wildcard listen address is checked through the loopback interface of the same family.
fn loopback_for(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        other => other,
    }
}
