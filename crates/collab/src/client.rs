//! A peer's connection to a host: the handshake, then a thread reading the host's messages
//! (picked up with `poll`, never waited for) and one writing ours.

use std::io::{self, BufReader, BufWriter, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use crate::wire::{self, MapInfo, Msg, Peer, Refusal, State, PROTOCOL};
use crate::auth;

/// The most a host's message may carry (a whole map when joining).
const MAX_FROM_HOST: u64 = 4 << 30;
/// A live update is bounded like an edit sent to the host; only joining takes a whole map.
const MAX_UPDATE: u64 = 512 << 20;
// A paused window or a slow connection must not grow queues without limit.
const QUEUE: usize = 8;
const CONNECT: Duration = Duration::from_secs(8);
/// A host not heard from this long is gone (it pings every few seconds).
const SILENCE: Duration = Duration::from_secs(45);
const PING_EVERY: Duration = Duration::from_secs(10);

/// What a host sends on joining.
#[derive(Debug, Clone)]
pub struct Welcome {
    pub you: Peer,
    pub map: MapInfo,
    pub ids: (i64, i64),
    pub peers: Vec<Peer>,
    pub states: Vec<State>,
}

#[derive(Debug)]
pub enum JoinError {
    /// Nobody answered, or the connection broke.
    Io(io::Error),
    Refused(Refusal),
    /// Something that is not an openMapper host.
    NotAHost,
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinError::Io(e) => write!(f, "{e}"),
            JoinError::Refused(r) => write!(f, "turned away: {r:?}"),
            JoinError::NotAHost => write!(f, "not an openMapper host"),
        }
    }
}

/// What came from the host since the last `poll`.
#[derive(Debug)]
pub enum Event {
    Msg(Msg),
    /// The connection is gone (why).
    Closed(String),
}

/// A connection to a host; dropping it leaves.
pub struct Client {
    out: SyncSender<Msg>,
    events: Receiver<Event>,
    stream: TcpStream,
}

impl Client {
    /// Join `addr` (host:port). Blocks until the host welcomes or refuses (a few seconds at
    /// most for a refusal; a big map takes as long as it takes to come).
    pub fn connect(addr: &str, name: &str, password: &str, fresh: bool) -> Result<(Client, Welcome), JoinError> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
        let mut stream = None;
        for a in addr.to_socket_addrs().map_err(JoinError::Io)? {
            match TcpStream::connect_timeout(&a, CONNECT) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = e,
            }
        }
        let stream = stream.ok_or(JoinError::Io(last))?;
        stream.set_nodelay(true).map_err(JoinError::Io)?;
        stream.set_read_timeout(Some(SILENCE)).map_err(JoinError::Io)?;
        let mut r = BufReader::new(stream.try_clone().map_err(JoinError::Io)?);
        let nonce = match wire::read(&mut r, 0) {
            Ok(Msg::Challenge { protocol, .. }) if protocol != PROTOCOL => return Err(JoinError::Refused(Refusal::Protocol(protocol))),
            Ok(Msg::Challenge { nonce, .. }) => nonce,
            Ok(_) => return Err(JoinError::NotAHost),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => return Err(JoinError::NotAHost),
            Err(e) => return Err(JoinError::Io(e)),
        };
        let hello = Msg::Hello { protocol: PROTOCOL, name: name.to_string(), proof: auth::proof(&nonce, password), fresh };
        (&stream).write_all(&wire::encode(&hello)).map_err(JoinError::Io)?;
        let welcome = match wire::read(&mut r, MAX_FROM_HOST).map_err(JoinError::Io)? {
            Msg::Welcome { you, map, ids, peers, states } => Welcome { you, map, ids, peers, states },
            Msg::Refused(why) => return Err(JoinError::Refused(why)),
            _ => return Err(JoinError::NotAHost),
        };
        let (out, queue) = mpsc::sync_channel::<Msg>(QUEUE);
        let (tx, events) = mpsc::sync_channel(QUEUE);
        let w = stream.try_clone().map_err(JoinError::Io)?;
        std::thread::Builder::new().name("collab write".into()).spawn(move || write_loop(w, queue)).map_err(JoinError::Io)?;
        std::thread::Builder::new()
            .name("collab read".into())
            .spawn(move || loop {
                match wire::read(&mut r, MAX_UPDATE) {
                    Ok(Msg::Ping) => {}
                    Ok(m) => {
                        if tx.send(Event::Msg(m)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Closed(e.to_string()));
                        break;
                    }
                }
            })
            .map_err(JoinError::Io)?;
        Ok((Client { out, events, stream }, welcome))
    }

    /// Send a message (queued: never waits).
    pub fn send(&self, m: Msg) {
        if self.out.try_send(m).is_err() {
            // Never block the editor or silently drop an edit. Closing is reported by the
            // reader; the editor retains its local document for saving as a copy.
            let _ = self.stream.shutdown(Shutdown::Both);
        }
    }

    /// What came since the last call.
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn write_loop(w: TcpStream, queue: Receiver<Msg>) {
    let mut w = BufWriter::new(w);
    loop {
        let m = match queue.recv_timeout(PING_EVERY) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => Msg::Ping,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if w.write_all(&wire::encode(&m)).and_then(|_| w.flush()).is_err() {
            break;
        }
    }
    let _ = w.get_ref().shutdown(Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_send_queue_closes_instead_of_blocking_or_losing_an_edit() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut remote, _) = listener.accept().unwrap();
        remote.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let (out, queue) = mpsc::sync_channel(QUEUE);
        let (_tx, events) = mpsc::sync_channel(QUEUE);
        let client = Client { out, events, stream };
        for _ in 0..QUEUE {
            client.send(Msg::NeedIds);
        }
        client.send(Msg::NeedIds);
        assert_eq!(queue.try_iter().count(), QUEUE);
        let mut byte = [0];
        assert_eq!(std::io::Read::read(&mut remote, &mut byte).unwrap(), 0);
    }
}
