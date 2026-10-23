//! The host: keeps every part of the map with its version, takes or turns down edits and
//! passes them on.
//!
//! One thread (the hub) owns the parts and handles everything in the order it came; each
//! peer has a thread reading from it and one writing to it. A peer whose writes pile up (a
//! slow connection) is let go rather than holding the others up.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufReader, BufWriter, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::wire::{self, MapInfo, Msg, Part, Peer, Refusal, State, PROTOCOL};
use crate::{auth, ID_BLOCK};

/// The most a peer's message may carry (its parts unpacked).
const MAX_FROM_PEER: u64 = 512 << 20;
/// Messages waiting to be written to one peer before it is let go.
const QUEUE: usize = 256;
/// A peer not heard from this long is gone (they send a ping every few seconds).
const SILENCE: Duration = Duration::from_secs(45);
const HANDSHAKE: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(10);
/// The longest name taken.
const MAX_NAME: usize = 32;
/// The longest chat line passed on.
const MAX_SAY: usize = 500;

const COLORS: [[u8; 3]; 8] = [[230, 80, 70], [60, 150, 240], [80, 190, 90], [240, 170, 40], [170, 100, 230], [40, 190, 190], [240, 110, 180], [150, 150, 150]];

pub struct HostConfig {
    pub bind: SocketAddr,
    /// Empty: anybody who reaches the port may join.
    pub password: String,
    pub map: MapInfo,
    /// The first IDCode not used yet (the map's `next_id_code`).
    pub next_id: i64,
    pub max_peers: usize,
}

/// A running host; it stops when this is dropped.
pub struct Host {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Host {
    /// Listen and keep `states` (each part of the map at version 1).
    pub fn start(cfg: HostConfig, states: Vec<State>) -> io::Result<Host> {
        let listener = TcpListener::bind(cfg.bind)?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let password = Arc::new(cfg.password.clone());
        let hub = Hub::new(cfg, states);
        let accept = {
            let stop = stop.clone();
            std::thread::Builder::new().name("collab accept".into()).spawn(move || accept_loop(listener, tx, password, stop))?
        };
        let hub = {
            let stop = stop.clone();
            std::thread::Builder::new().name("collab hub".into()).spawn(move || hub.run(rx, stop))?
        };
        log::info!("multiplayer: hosting on {addr}");
        Ok(Host { addr, stop, threads: vec![accept, hub] })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        log::info!("multiplayer: host stopped");
    }
}

enum Event {
    Joined { conn: u64, name: String, fresh: bool, stream: TcpStream },
    Got { conn: u64, msg: Msg },
    Left { conn: u64 },
}

fn accept_loop(listener: TcpListener, tx: Sender<Event>, password: Arc<String>, stop: Arc<AtomicBool>) {
    let mut next = 1u64;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, from)) => {
                let (tx, password, conn) = (tx.clone(), password.clone(), next);
                next += 1;
                let _ = std::thread::Builder::new().name("collab peer".into()).spawn(move || {
                    if let Err(e) = serve(stream, conn, &tx, &password) {
                        log::info!("multiplayer: {from}: {e}");
                    }
                    let _ = tx.send(Event::Left { conn });
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                log::warn!("multiplayer: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// One peer's connection: the handshake, then its messages to the hub.
fn serve(stream: TcpStream, conn: u64, tx: &Sender<Event>, password: &str) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE))?;
    let nonce = auth::nonce();
    let mut w = stream.try_clone()?;
    w.write_all(&wire::encode(&Msg::Challenge { protocol: PROTOCOL, nonce: nonce.clone() }))?;
    let mut r = BufReader::new(stream.try_clone()?);
    let refuse = |w: &mut TcpStream, why: Refusal| {
        let _ = w.write_all(&wire::encode(&Msg::Refused(why.clone())));
        io::Error::other(format!("turned away: {why:?}"))
    };
    let (name, fresh) = match wire::read(&mut r, 0)? {
        Msg::Hello { protocol, .. } if protocol != PROTOCOL => return Err(refuse(&mut w, Refusal::Protocol(PROTOCOL))),
        Msg::Hello { name, proof, fresh, .. } => {
            if !auth::check(&nonce, password, &proof) {
                // (a moment's wait: passwords are not tried quickly)
                std::thread::sleep(Duration::from_millis(500));
                return Err(refuse(&mut w, Refusal::Password));
            }
            let name: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
            if name.is_empty() || name.chars().count() > MAX_NAME {
                return Err(refuse(&mut w, Refusal::Name));
            }
            (name, fresh)
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "no hello")),
    };
    stream.set_read_timeout(Some(SILENCE))?;
    if tx.send(Event::Joined { conn, name, fresh, stream: stream.try_clone()? }).is_err() {
        return Ok(());
    }
    loop {
        let msg = wire::read(&mut r, MAX_FROM_PEER)?;
        if tx.send(Event::Got { conn, msg }).is_err() {
            return Ok(());
        }
    }
}

struct Member {
    peer: Peer,
    out: SyncSender<Arc<Vec<u8>>>,
    stream: TcpStream,
}

struct Hub {
    cfg: HostConfig,
    parts: BTreeMap<Part, (u64, Option<Vec<u8>>)>,
    /// The last version given out (every new state gets the next).
    clock: u64,
    members: HashMap<u64, Member>,
    next_peer: u32,
}

impl Hub {
    fn new(cfg: HostConfig, states: Vec<State>) -> Hub {
        let parts = states.into_iter().map(|s| (s.part, (1, s.data))).collect();
        Hub { cfg, parts, clock: 1, members: HashMap::new(), next_peer: 1 }
    }

    fn run(mut self, rx: Receiver<Event>, stop: Arc<AtomicBool>) {
        let mut pinged = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(e) => self.handle(e),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if pinged.elapsed() > PING_EVERY {
                pinged = Instant::now();
                self.broadcast(&Msg::Ping, None);
            }
        }
        for m in self.members.values() {
            let _ = m.stream.shutdown(Shutdown::Both);
        }
    }

    fn handle(&mut self, e: Event) {
        match e {
            Event::Joined { conn, name, fresh, stream } => self.join(conn, name, fresh, stream),
            Event::Left { conn } => {
                if let Some(m) = self.members.remove(&conn) {
                    log::info!("multiplayer: {} left", m.peer.name);
                    let _ = m.stream.shutdown(Shutdown::Both);
                    self.send_peers();
                }
            }
            Event::Got { conn, msg } => self.got(conn, msg),
        }
    }

    fn join(&mut self, conn: u64, name: String, fresh: bool, stream: TcpStream) {
        if self.members.len() >= self.cfg.max_peers {
            let mut s = &stream;
            let _ = s.write_all(&wire::encode(&Msg::Refused(Refusal::Full)));
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        let id = self.next_peer;
        self.next_peer += 1;
        let peer = Peer { id, name, color: COLORS[(id as usize - 1) % COLORS.len()] };
        let (out, queue) = mpsc::sync_channel::<Arc<Vec<u8>>>(QUEUE);
        let Ok(w) = stream.try_clone() else { return };
        let _ = std::thread::Builder::new().name("collab write".into()).spawn(move || {
            let mut w = BufWriter::new(w);
            for bytes in queue {
                if w.write_all(&bytes).and_then(|_| w.flush()).is_err() {
                    let _ = w.get_ref().shutdown(Shutdown::Both);
                    break;
                }
            }
        });
        let states = self.parts.iter().map(|(&part, (version, data))| State { part, version: *version, data: if fresh { data.clone() } else { None } }).collect();
        let ids = self.grant_ids();
        let mut peers: Vec<Peer> = self.members.values().map(|m| m.peer.clone()).collect();
        peers.push(peer.clone());
        peers.sort_by_key(|p| p.id);
        let welcome = Msg::Welcome { you: peer.clone(), map: self.cfg.map.clone(), ids, peers, states };
        log::info!("multiplayer: {} joined", peer.name);
        self.members.insert(conn, Member { peer, out, stream });
        self.send(conn, &welcome);
        self.send_peers();
    }

    fn got(&mut self, conn: u64, msg: Msg) {
        let Some(author) = self.members.get(&conn).map(|m| m.peer.id) else { return };
        match msg {
            Msg::Edit { seq, base, states } => self.edit(conn, author, seq, base, states),
            Msg::Presence(presence) => self.broadcast(&Msg::Seen { peer: author, presence }, Some(conn)),
            Msg::Say(text) => {
                let text: String = text.chars().filter(|c| !c.is_control()).take(MAX_SAY).collect();
                if !text.trim().is_empty() {
                    self.broadcast(&Msg::Said { peer: author, text }, None);
                }
            }
            Msg::NeedIds => {
                let (a, b) = self.grant_ids();
                self.send(conn, &Msg::Ids(a, b));
            }
            Msg::Ping => {}
            // (what only a host sends, or a second hello)
            _ => self.drop_member(conn),
        }
    }

    /// Take the edit if every part it changes is still at the version it was changed from.
    fn edit(&mut self, conn: u64, author: u32, seq: u64, base: Vec<(Part, u64)>, states: Vec<State>) {
        let base: HashMap<Part, u64> = base.into_iter().collect();
        let now = |p: &Part| self.parts.get(p).map(|v| v.0).unwrap_or(0);
        let mut stale: Vec<Part> = states.iter().filter(|s| base.get(&s.part) != Some(&now(&s.part))).map(|s| s.part).collect();
        stale.sort();
        stale.dedup();
        if !stale.is_empty() {
            let states = stale.iter().map(|&part| State { part, version: now(&part), data: self.parts.get(&part).and_then(|p| p.1.clone()) }).collect();
            self.send(conn, &Msg::Rejected { seq, states });
            return;
        }
        let mut taken = Vec::with_capacity(states.len());
        for s in states {
            self.clock += 1;
            self.parts.insert(s.part, (self.clock, s.data.clone()));
            taken.push(State { part: s.part, version: self.clock, data: s.data });
        }
        // (the author has the bytes: only the new versions go back)
        let versions = taken.iter().map(|s| State { part: s.part, version: s.version, data: None }).collect();
        self.send(conn, &Msg::Applied { author, seq: Some(seq), states: versions });
        self.broadcast(&Msg::Applied { author, seq: None, states: taken }, Some(conn));
    }

    fn grant_ids(&mut self) -> (i64, i64) {
        let from = self.cfg.next_id.max(1);
        self.cfg.next_id = from + ID_BLOCK;
        (from, from + ID_BLOCK)
    }

    fn send_peers(&mut self) {
        let mut peers: Vec<Peer> = self.members.values().map(|m| m.peer.clone()).collect();
        peers.sort_by_key(|p| p.id);
        self.broadcast(&Msg::Peers(peers), None);
    }

    fn send(&mut self, conn: u64, m: &Msg) {
        self.send_bytes(conn, Arc::new(wire::encode(m)));
    }

    fn send_bytes(&mut self, conn: u64, bytes: Arc<Vec<u8>>) {
        let Some(member) = self.members.get(&conn) else { return };
        match member.out.try_send(bytes) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                log::warn!("multiplayer: {} cannot keep up, let go", member.peer.name);
                self.drop_member(conn);
            }
            Err(TrySendError::Disconnected(_)) => self.drop_member(conn),
        }
    }

    fn broadcast(&mut self, m: &Msg, except: Option<u64>) {
        let bytes = Arc::new(wire::encode(m));
        let conns: Vec<u64> = self.members.keys().copied().filter(|c| Some(*c) != except).collect();
        for c in conns {
            self.send_bytes(c, bytes.clone());
        }
    }

    fn drop_member(&mut self, conn: u64) {
        if let Some(m) = self.members.remove(&conn) {
            let _ = m.stream.shutdown(Shutdown::Both);
            self.send_peers();
        }
    }
}
