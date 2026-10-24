//! A peer's connection to a host: the handshake, then a thread reading the host's messages
//! (picked up with `poll`, never waited for) and one writing ours.

use std::io::{self, BufReader, BufWriter, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::wire::{self, MapInfo, Msg, Peer, Refusal, State, PROTOCOL};
use crate::auth;

/// The most a host's message may carry (a whole map when joining).
const MAX_FROM_HOST: u64 = 4 << 30;
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
    out: Sender<Msg>,
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
        let (nonce, salt, work) = match wire::read(&mut r, 0) {
            Ok(Msg::Challenge { protocol, .. }) if protocol != PROTOCOL => return Err(JoinError::Refused(Refusal::Protocol(protocol))),
            // (a host asking for more work than any would is not one to wait for)
            Ok(Msg::Challenge { work, .. }) if work > auth::MAX_WORK => return Err(JoinError::NotAHost),
            Ok(Msg::Challenge { nonce, salt, work, .. }) => (nonce, salt, work),
            Ok(_) => return Err(JoinError::NotAHost),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => return Err(JoinError::NotAHost),
            Err(e) => return Err(JoinError::Io(e)),
        };
        let hello = Msg::Hello { protocol: PROTOCOL, name: name.to_string(), proof: auth::proof(&auth::derive(password, &salt, work), &nonce), fresh };
        (&stream).write_all(&wire::encode(&hello)).map_err(JoinError::Io)?;
        let welcome = match wire::read(&mut r, MAX_FROM_HOST).map_err(JoinError::Io)? {
            Msg::Welcome { you, map, ids, peers, states } => Welcome { you, map, ids, peers, states },
            Msg::Refused(why) => return Err(JoinError::Refused(why)),
            _ => return Err(JoinError::NotAHost),
        };
        let (out, queue) = mpsc::channel::<Msg>();
        let (tx, events) = mpsc::channel();
        let w = stream.try_clone().map_err(JoinError::Io)?;
        std::thread::Builder::new().name("collab write".into()).spawn(move || write_loop(w, queue)).map_err(JoinError::Io)?;
        std::thread::Builder::new()
            .name("collab read".into())
            .spawn(move || loop {
                match wire::read(&mut r, MAX_FROM_HOST) {
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
        let _ = self.out.send(m);
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
