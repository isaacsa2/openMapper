//! The host: keeps every part of the map with its version, takes or turns down edits and
//! passes them on.
//!
//! One thread (the hub) owns the parts and handles everything in the order it came; each
//! peer has a thread reading from it and one writing to it. A peer whose writes pile up (a
//! slow connection) is let go rather than holding the others up.
//!
//! The port may be reached from the LAN or, through a tunnel, from anywhere, so the host
//! takes nothing unbounded: connections and handshakes are counted and capped, a handshake
//! has a deadline as a whole, password answers are checked one at a time with a growing wait
//! after each wrong one (whoever sends them: through a tunnel every peer comes from the
//! loopback), and what an edit may add is bounded in bytes, in parts and in where its tiles
//! lie.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::wire::{self, MapInfo, Msg, Part, Peer, Refusal, State, Why, PROTOCOL};
use crate::{auth, ID_BLOCK};

/// The most a peer's message may carry (its parts unpacked).
const MAX_FROM_PEER: u64 = 64 << 20;
/// The most parts one edit may change.
const MAX_EDIT_PARTS: usize = 4096;
/// Messages waiting to be written to one peer before it is let go.
const QUEUE: usize = 256;
/// A peer not heard from this long is gone (they send a ping every few seconds).
const SILENCE: Duration = Duration::from_secs(45);
const PING_EVERY: Duration = Duration::from_secs(10);
/// Connections still in their handshake at once (more are closed at once).
pub const MAX_HANDSHAKES: usize = 8;
/// The wait after the first wrong password, doubled for each next one, at most `MAX_WAIT`;
/// forgotten after `FORGIVE` without a wrong one.
const FIRST_WAIT: Duration = Duration::from_millis(250);
const MAX_WAIT: Duration = Duration::from_secs(4);
const FORGIVE: Duration = Duration::from_secs(60);
/// The longest name taken.
const MAX_NAME: usize = 32;
/// The longest chat line passed on.
const MAX_SAY: usize = 500;
/// What of a presence is passed on: selected things, letters of the tool.
const MAX_SELECTED: usize = 200;
const MAX_TOOL: usize = 32;

const COLORS: [[u8; 3]; 8] = [[230, 80, 70], [60, 150, 240], [80, 190, 90], [240, 170, 40], [170, 100, 230], [40, 190, 190], [240, 110, 180], [150, 150, 150]];

pub struct HostConfig {
    pub bind: SocketAddr,
    pub password: String,
    /// No password at all: anybody who reaches the port may join. Never by default: the
    /// host does not start with an empty password unless this says so.
    pub open: bool,
    /// The work factor of the password's key (`auth::WORK`; tests take less).
    pub work: u32,
    pub map: MapInfo,
    /// The first IDCode not used yet (the map's `next_id_code`).
    pub next_id: i64,
    pub max_peers: usize,
    /// How long a connection has for its handshake, all of it.
    pub handshake: Duration,
    /// What the host holds at most beyond the map it started with: bytes, parts.
    pub room_bytes: u64,
    pub room_parts: usize,
    /// How many tiles beyond the map's tiles at the start new ones may lie.
    pub margin: i32,
}

impl Default for HostConfig {
    fn default() -> HostConfig {
        HostConfig {
            bind: (std::net::Ipv4Addr::LOCALHOST, crate::DEFAULT_PORT).into(),
            password: String::new(),
            open: false,
            work: auth::WORK,
            map: MapInfo { folder: String::new(), name: String::new() },
            next_id: 1,
            max_peers: 16,
            handshake: Duration::from_secs(10),
            room_bytes: 512 << 20,
            room_parts: 4096,
            margin: 32,
        }
    }
}

/// A running host; it stops when this is dropped.
pub struct Host {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    open: Arc<AtomicUsize>,
    threads: Vec<JoinHandle<()>>,
}

impl Host {
    /// Listen and keep `states` (each part of the map at version 1).
    pub fn start(cfg: HostConfig, states: Vec<State>) -> io::Result<Host> {
        if cfg.password.is_empty() && !cfg.open {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a host needs a password unless it is opened to anybody on purpose"));
        }
        let listener = TcpListener::bind(cfg.bind)?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let open = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let salt = auth::nonce();
        // (derived once: an answer costs the host one HMAC to check)
        let key = auth::derive(&cfg.password, &salt, cfg.work);
        let throttle = Mutex::new(Throttle { wrong: 0, until: Instant::now(), last: Instant::now() });
        let gate = Arc::new(Gate { salt, work: cfg.work, key, throttle, handshake: cfg.handshake, handshaking: AtomicUsize::new(0), max_open: cfg.max_peers + MAX_HANDSHAKES });
        let hub = Hub::new(cfg, states);
        let accept = {
            let (stop, open) = (stop.clone(), open.clone());
            std::thread::Builder::new().name("collab accept".into()).spawn(move || accept_loop(listener, tx, gate, open, stop))?
        };
        let hub = {
            let stop = stop.clone();
            std::thread::Builder::new().name("collab hub".into()).spawn(move || hub.run(rx, stop))?
        };
        log::info!("multiplayer: hosting on {addr}");
        Ok(Host { addr, stop, open, threads: vec![accept, hub] })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Connections open now (in their handshake or joined).
    pub fn connections(&self) -> usize {
        self.open.load(Ordering::Relaxed)
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

/// What every handshake shares: the password's key and what keeps guessing slow.
struct Gate {
    salt: String,
    work: u32,
    key: auth::Key,
    throttle: Mutex<Throttle>,
    handshake: Duration,
    handshaking: AtomicUsize,
    max_open: usize,
}

struct Throttle {
    /// Wrong answers in a row, and until when the next answer waits.
    wrong: u32,
    until: Instant,
    last: Instant,
}

impl Gate {
    /// Check an answer: one at a time, each after the wait the wrong ones before it left.
    fn check(&self, nonce: &str, answer: &str) -> bool {
        let mut t = self.throttle.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if t.wrong > 0 && now.duration_since(t.last) > FORGIVE {
            t.wrong = 0;
        }
        if t.until > now {
            std::thread::sleep(t.until - now);
        }
        let ok = auth::check(&self.key, nonce, answer);
        if ok {
            t.wrong = 0;
        } else {
            t.wrong += 1;
            t.last = Instant::now();
            t.until = t.last + FIRST_WAIT.saturating_mul(1 << (t.wrong - 1).min(8)).min(MAX_WAIT);
        }
        ok
    }
}

enum Event {
    Joined { conn: u64, name: String, fresh: bool, stream: TcpStream },
    Got { conn: u64, msg: Msg },
    Left { conn: u64 },
}

fn accept_loop(listener: TcpListener, tx: Sender<Event>, gate: Arc<Gate>, open: Arc<AtomicUsize>, stop: Arc<AtomicBool>) {
    let mut next = 1u64;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, from)) => {
                // (over the caps: closed at once, no thread for it)
                if gate.handshaking.load(Ordering::Relaxed) >= MAX_HANDSHAKES || open.load(Ordering::Relaxed) >= gate.max_open {
                    log::debug!("multiplayer: {from}: too many connections, closed");
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                open.fetch_add(1, Ordering::Relaxed);
                gate.handshaking.fetch_add(1, Ordering::Relaxed);
                let (t, g, o, conn) = (tx.clone(), gate.clone(), open.clone(), next);
                next += 1;
                let spawned = std::thread::Builder::new().name("collab peer".into()).spawn(move || {
                    if let Err(e) = serve(stream, conn, &t, &g) {
                        log::info!("multiplayer: {from}: {e}");
                    }
                    let _ = t.send(Event::Left { conn });
                    o.fetch_sub(1, Ordering::Relaxed);
                });
                if spawned.is_err() {
                    open.fetch_sub(1, Ordering::Relaxed);
                    gate.handshaking.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                log::warn!("multiplayer: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// A stream read until a deadline as a whole (a peer sending a byte now and then does not
/// keep its handshake open).
struct Deadline<'a> {
    s: &'a TcpStream,
    until: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "handshake took too long"));
        }
        self.s.set_read_timeout(Some(left))?;
        (&mut &*self.s).read(buf)
    }
}

/// One peer's connection: the handshake, then its messages to the hub.
fn serve(stream: TcpStream, conn: u64, tx: &Sender<Event>, gate: &Gate) -> io::Result<()> {
    let shake = handshake(&stream, gate);
    gate.handshaking.fetch_sub(1, Ordering::Relaxed);
    let (name, fresh) = shake?;
    stream.set_read_timeout(Some(SILENCE))?;
    if tx.send(Event::Joined { conn, name, fresh, stream: stream.try_clone()? }).is_err() {
        return Ok(());
    }
    let mut r = BufReader::new(stream.try_clone()?);
    loop {
        let msg = wire::read(&mut r, MAX_FROM_PEER)?;
        if tx.send(Event::Got { conn, msg }).is_err() {
            return Ok(());
        }
    }
}

/// The challenge and the hello: the peer's name and whether it wants the map's parts.
fn handshake(stream: &TcpStream, gate: &Gate) -> io::Result<(String, bool)> {
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    let until = Instant::now() + gate.handshake;
    let nonce = auth::nonce();
    let mut w = stream;
    w.write_all(&wire::encode(&Msg::Challenge { protocol: PROTOCOL, nonce: nonce.clone(), salt: gate.salt.clone(), work: gate.work }))?;
    let refuse = |why: Refusal| {
        let _ = (&mut &*stream).write_all(&wire::encode(&Msg::Refused(why.clone())));
        io::Error::other(format!("turned away: {why:?}"))
    };
    // (read unbuffered: nothing past the hello is taken from the stream)
    match wire::read(&mut Deadline { s: stream, until }, 0)? {
        Msg::Hello { protocol, .. } if protocol != PROTOCOL => Err(refuse(Refusal::Protocol(PROTOCOL))),
        Msg::Hello { name, proof, fresh, .. } => {
            // (a wrong answer ends the connection: the next guess needs a new one, and waits)
            if !gate.check(&nonce, &proof) {
                return Err(refuse(Refusal::Password));
            }
            let name: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
            if name.is_empty() || name.chars().count() > MAX_NAME {
                return Err(refuse(Refusal::Name));
            }
            Ok((name, fresh))
        }
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "no hello")),
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
    /// The bytes the parts come to, and the most they may.
    held: u64,
    max_bytes: u64,
    max_parts: usize,
    /// Where tiles may lie (x from, x to, y from, y to: both ends included).
    tiles: (i32, i32, i32, i32),
}

impl Hub {
    fn new(cfg: HostConfig, states: Vec<State>) -> Hub {
        let parts: BTreeMap<Part, (u64, Option<Vec<u8>>)> = states.into_iter().map(|s| (s.part, (1, s.data))).collect();
        let held = parts.values().map(|p| size(&p.1)).sum::<u64>();
        let keys = parts.keys().filter_map(|p| match p {
            Part::Tile(x, y) => Some((*x, *y)),
            _ => None,
        });
        let (mut x0, mut x1, mut y0, mut y1) = (0, 0, 0, 0);
        for (i, (x, y)) in keys.enumerate() {
            if i == 0 {
                (x0, x1, y0, y1) = (x, x, y, y);
            }
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        let m = cfg.margin.max(0);
        let tiles = (x0.saturating_sub(m), x1.saturating_add(m), y0.saturating_sub(m), y1.saturating_add(m));
        let (max_bytes, max_parts) = (held + cfg.room_bytes, parts.len() + cfg.room_parts);
        Hub { cfg, parts, clock: 1, members: HashMap::new(), next_peer: 1, held, max_bytes, max_parts, tiles }
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
            Msg::Presence(mut presence) => {
                presence.selected.truncate(MAX_SELECTED);
                presence.tool = presence.tool.chars().filter(|c| !c.is_control()).take(MAX_TOOL).collect();
                self.broadcast(&Msg::Seen { peer: author, presence }, Some(conn));
            }
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

    /// Take the edit if every part it changes is still at the version it was changed from, is
    /// one the host keeps (a tile within reach of the map) and what the host holds stays within
    /// its bounds; else none of it.
    fn edit(&mut self, conn: u64, author: u32, seq: u64, base: Vec<(Part, u64)>, states: Vec<State>) {
        let base: HashMap<Part, u64> = base.into_iter().collect();
        // (a part twice: the last one counts)
        let mut by_part: BTreeMap<Part, Option<Vec<u8>>> = BTreeMap::new();
        for s in states {
            by_part.insert(s.part, s.data);
        }
        let now = |p: &Part| self.parts.get(p).map(|v| v.0).unwrap_or(0);
        let why = if by_part.len() > MAX_EDIT_PARTS {
            Some(Why::TooMuch)
        } else if by_part.keys().any(|p| !self.keeps(p)) {
            Some(Why::NotThere)
        } else if by_part.keys().any(|p| base.get(p) != Some(&now(p))) {
            Some(Why::Stale)
        } else {
            let gone: u64 = by_part.keys().map(|p| self.parts.get(p).map(|v| size(&v.1)).unwrap_or(0)).sum();
            let came: u64 = by_part.values().map(size).sum();
            let new_parts = by_part.keys().filter(|p| !self.parts.contains_key(p)).count();
            (self.held - gone + came > self.max_bytes || self.parts.len() + new_parts > self.max_parts).then_some(Why::TooMuch)
        };
        if let Some(why) = why {
            log::info!("multiplayer: an edit turned down: {why:?}");
            // (every part the edit changed, as it is now: the sender puts back all of them, the
            // ones that were fine too, since none of the edit was taken)
            let states = by_part.keys().map(|&part| State { part, version: now(&part), data: self.parts.get(&part).and_then(|p| p.1.clone()) }).collect();
            self.send(conn, &Msg::Rejected { seq, why, states });
            return;
        }
        let mut taken = Vec::with_capacity(by_part.len());
        for (part, data) in by_part {
            self.clock += 1;
            self.held += size(&data);
            if let Some(old) = self.parts.insert(part, (self.clock, data.clone())) {
                self.held -= size(&old.1);
            }
            taken.push(State { part, version: self.clock, data });
        }
        // (the author has the bytes: only the new versions go back)
        let versions = taken.iter().map(|s| State { part: s.part, version: s.version, data: None }).collect();
        self.send(conn, &Msg::Applied { author, seq: Some(seq), states: versions });
        self.broadcast(&Msg::Applied { author, seq: None, states: taken }, Some(conn));
    }

    /// Whether the host keeps this part: a tile within reach of the map, or one of the parts
    /// it started with.
    fn keeps(&self, p: &Part) -> bool {
        match *p {
            Part::Tile(x, y) => (self.tiles.0..=self.tiles.1).contains(&x) && (self.tiles.2..=self.tiles.3).contains(&y),
            _ => self.parts.contains_key(p),
        }
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

fn size(d: &Option<Vec<u8>>) -> u64 {
    d.as_ref().map(|d| d.len() as u64).unwrap_or(0)
}
