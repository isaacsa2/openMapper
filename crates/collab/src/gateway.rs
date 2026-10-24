//! The way in through a tunnel: a WebSocket at `/collab` whose bytes go to the host's TCP
//! port and back. A Cloudflare quick tunnel (openOMSI's `omsi_net::tunnel`) carries HTTP and
//! WebSockets only; in front of this, it reaches the host from anywhere, encrypted, with no
//! port to open. Peers come in with openOMSI's `omsi_net::ws::tcp_forward`, which gives them a
//! local TCP port to join as on the LAN.
//!
//! It listens on the loopback only: what comes from outside comes through the tunnel.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, WebSocket};

/// The path peers open.
pub const PATH: &str = "/collab";
/// Connections carried at once.
const MAX_CONNS: usize = 32;

/// A running gateway; it stops when this is dropped.
pub struct Gateway {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Gateway {
    /// Listen on a free loopback port and carry each WebSocket to `target`.
    pub fn start(target: SocketAddr) -> io::Result<Gateway> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let thread = std::thread::Builder::new().name("collab gateway".into()).spawn(move || {
            let open = Arc::new(AtomicUsize::new(0));
            while !st.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((s, _)) if open.load(Ordering::Relaxed) < MAX_CONNS => {
                        let (st, open) = (st.clone(), open.clone());
                        open.fetch_add(1, Ordering::Relaxed);
                        let _ = std::thread::Builder::new().name("collab gateway conn".into()).spawn(move || {
                            if let Err(e) = carry(s, target, &st) {
                                log::info!("multiplayer gateway: {e}");
                            }
                            open.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                    Err(e) => {
                        log::warn!("multiplayer gateway: {e}");
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        })?;
        Ok(Gateway { addr, stop, thread: Some(thread) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One WebSocket: its binary messages to the TCP side, what the TCP side says back as
/// binary messages.
fn carry(s: TcpStream, target: SocketAddr, stop: &AtomicBool) -> Result<(), String> {
    s.set_nonblocking(false).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let mut ws = tungstenite::accept_hdr(s, our_path).map_err(|e| e.to_string())?;
    let mut tcp = TcpStream::connect_timeout(&target, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
    ws.get_mut().set_read_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
    tcp.set_nonblocking(true).map_err(|e| e.to_string())?;
    let r = pump(&mut ws, &mut tcp, stop);
    let _ = tcp.shutdown(std::net::Shutdown::Both);
    r
}

/// The handshake goes on for `PATH` only.
#[allow(clippy::result_large_err)] // (tungstenite's callback)
fn our_path(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    if req.uri().path() == PATH {
        Ok(resp)
    } else {
        let mut e = ErrorResponse::new(Some("not here".into()));
        *e.status_mut() = tungstenite::http::StatusCode::NOT_FOUND;
        Err(e)
    }
}

fn pump(ws: &mut WebSocket<TcpStream>, tcp: &mut TcpStream, stop: &AtomicBool) -> Result<(), String> {
    let mut buf = vec![0u8; 64 * 1024];
    while !stop.load(Ordering::Relaxed) {
        let mut idle = true;
        loop {
            match ws.read() {
                Ok(Message::Binary(d)) => {
                    idle = false;
                    tcp.set_nonblocking(false).map_err(|e| e.to_string())?;
                    let w = tcp.write_all(&d);
                    tcp.set_nonblocking(true).map_err(|e| e.to_string())?;
                    w.map_err(|e| e.to_string())?;
                }
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(()),
                Err(e) => return Err(e.to_string()),
            }
        }
        // (a few chunks of the TCP side per turn: a whole map goes out when somebody joins)
        for _ in 0..32 {
            match tcp.read(&mut buf) {
                Ok(0) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return Ok(());
                }
                Ok(n) => {
                    idle = false;
                    ws.write(Message::Binary(buf[..n].to_vec().into())).map_err(|e| e.to_string())?;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        match ws.flush() {
            Ok(()) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.to_string()),
        }
        if idle {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}

/// The WebSocket address for what a peer typed or pasted, when it names a tunnel
/// (`https://…`, `wss://…` or a bare `….trycloudflare.com`); None: a LAN address
/// (`host:port`).
pub fn tunnel_url(typed: &str) -> Option<String> {
    let t = typed.trim().trim_end_matches('/');
    let host = if let Some(rest) = t.strip_prefix("https://").or_else(|| t.strip_prefix("wss://")) {
        rest
    } else if t.ends_with(".trycloudflare.com") {
        t
    } else {
        return None;
    };
    let host = host.split('/').next().unwrap_or(host);
    (!host.is_empty()).then(|| format!("wss://{host}{PATH}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_addresses_are_told_from_lan_ones() {
        assert_eq!(tunnel_url("https://quiet-river.trycloudflare.com").as_deref(), Some("wss://quiet-river.trycloudflare.com/collab"));
        assert_eq!(tunnel_url(" https://quiet-river.trycloudflare.com/ ").as_deref(), Some("wss://quiet-river.trycloudflare.com/collab"));
        assert_eq!(tunnel_url("quiet-river.trycloudflare.com").as_deref(), Some("wss://quiet-river.trycloudflare.com/collab"));
        assert_eq!(tunnel_url("wss://quiet-river.trycloudflare.com/collab").as_deref(), Some("wss://quiet-river.trycloudflare.com/collab"));
        assert_eq!(tunnel_url("192.168.0.10:27560"), None);
        assert_eq!(tunnel_url("https://"), None);
    }
}
