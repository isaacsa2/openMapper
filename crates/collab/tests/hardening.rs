//! What a host takes from whoever reaches its port: connections, handshakes, guesses and
//! edits, each bounded.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use openmapper_collab::host::MAX_HANDSHAKES;
use openmapper_collab::{auth, wire, Client, Event, Host, HostConfig, JoinError, MapInfo, Msg, Part, Presence, Refusal, State, Why, PROTOCOL};

const PASSWORD: &str = "a long password";

fn cfg() -> HostConfig {
    HostConfig { bind: "127.0.0.1:0".parse().unwrap(), password: PASSWORD.into(), work: 10, map: MapInfo { folder: "T".into(), name: "T".into() }, max_peers: 4, ..Default::default() }
}

/// A map of tiles 0..3, 0, each of `bytes` bytes.
fn map(bytes: usize) -> Vec<State> {
    let mut v: Vec<State> = (0..3).map(|x| State { part: Part::Tile(x, 0), version: 0, data: Some(vec![1; bytes]) }).collect();
    v.push(State { part: Part::Global, version: 0, data: Some(b"global".to_vec()) });
    v
}

fn join(h: &Host, password: &str) -> Result<Client, JoinError> {
    Client::connect(&h.addr().to_string(), "Ana", password, true).map(|c| c.0)
}

fn until(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The next message that is not a peer list.
fn next(c: &Client) -> Msg {
    let mut got = None;
    until("nothing came", 5, || {
        for e in c.poll() {
            match e {
                Event::Msg(Msg::Peers(_)) => {}
                Event::Msg(m) => {
                    got = Some(m);
                    return true;
                }
                Event::Closed(why) => panic!("closed: {why}"),
            }
        }
        false
    });
    got.unwrap()
}

// 1. connections

#[test]
fn connections_that_never_say_hello_are_capped() {
    let h = Host::start(cfg(), map(10)).unwrap();
    // a flood of connections that say nothing
    let idle: Vec<TcpStream> = (0..60).map(|_| TcpStream::connect(h.addr()).unwrap()).collect();
    std::thread::sleep(Duration::from_millis(300));
    // (no more than the handshakes taken at once: the rest were closed, no thread each)
    assert!(h.connections() <= MAX_HANDSHAKES, "{} open", h.connections());
    drop(idle);
    until("the slots were never given back", 5, || h.connections() == 0);
    // and the host lets a peer in again
    assert!(join(&h, PASSWORD).is_ok());
}

#[test]
fn a_handshake_has_a_deadline_as_a_whole() {
    let h = Host::start(HostConfig { handshake: Duration::from_millis(800), ..cfg() }, map(10)).unwrap();
    let mut s = TcpStream::connect(h.addr()).unwrap();
    let mut r = s.try_clone().unwrap();
    let Msg::Challenge { .. } = wire::read(&mut r, 0).unwrap() else { panic!() };
    // a hello that never ends, a byte every 150 ms (each read well within any read timeout):
    // the host must close it while the bytes still come, soon after its deadline
    let mut bytes = b"OMMP\xe8\x03\x00\x00{\"Hello\":".to_vec();
    bytes.resize(100, b' ');
    let t = Instant::now();
    let mut closed = false;
    r.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
    for b in bytes {
        if s.write_all(&[b]).is_err() {
            closed = true;
            break;
        }
        let mut buf = [0u8; 16];
        match r.read(&mut buf) {
            Ok(0) => {
                closed = true;
                break;
            }
            Err(e) if !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                closed = true;
                break;
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(150));
        if t.elapsed() > Duration::from_secs(4) {
            break;
        }
    }
    assert!(closed && t.elapsed() < Duration::from_secs(2), "still open after {:?}", t.elapsed());
    until("the connection was kept", 3, || h.connections() == 0);
}

// 2. password guesses

#[test]
fn wrong_passwords_are_slowed_however_many_are_sent_at_once() {
    let h = Host::start(cfg(), map(10)).unwrap();
    let addr = h.addr().to_string();
    let t = Instant::now();
    let guesses: Vec<_> = (0..4)
        .map(|i| {
            let addr = addr.clone();
            std::thread::spawn(move || Client::connect(&addr, "Eve", &format!("guess {i}"), true).map(|_| ()))
        })
        .collect();
    for g in guesses {
        assert!(matches!(g.join().unwrap(), Err(JoinError::Refused(Refusal::Password))));
    }
    // (one at a time: after each wrong one the next waits 250 ms, then 500, then 1000)
    assert!(t.elapsed() >= Duration::from_millis(1700), "{:?}", t.elapsed());
    // the right one still gets in (after its wait)
    assert!(join(&h, PASSWORD).is_ok());
}

#[test]
fn a_wrong_answer_ends_the_connection() {
    let h = Host::start(cfg(), map(10)).unwrap();
    let mut s = TcpStream::connect(h.addr()).unwrap();
    let mut r = s.try_clone().unwrap();
    let Msg::Challenge { .. } = wire::read(&mut r, 0).unwrap() else { panic!() };
    s.write_all(&wire::encode(&Msg::Hello { protocol: PROTOCOL, name: "Eve".into(), proof: "00".repeat(32), fresh: true })).unwrap();
    assert_eq!(wire::read(&mut r, 0).unwrap(), Msg::Refused(Refusal::Password));
    // (no second try on it)
    let _ = s.write_all(&wire::encode(&Msg::Hello { protocol: PROTOCOL, name: "Eve".into(), proof: "11".repeat(32), fresh: true }));
    let mut b = [0u8; 1];
    r.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert!(matches!(r.read(&mut b), Ok(0) | Err(_)));
}

#[test]
fn an_answer_proves_the_password_under_the_hosts_salt_and_work() {
    // (the key of another salt or work factor is no answer)
    let h = Host::start(cfg(), map(10)).unwrap();
    let mut s = TcpStream::connect(h.addr()).unwrap();
    let mut r = s.try_clone().unwrap();
    let Msg::Challenge { nonce, salt, work, .. } = wire::read(&mut r, 0).unwrap() else { panic!() };
    assert_eq!(work, 10);
    let proof = auth::proof(&auth::derive(PASSWORD, &format!("{salt}x"), work), &nonce);
    s.write_all(&wire::encode(&Msg::Hello { protocol: PROTOCOL, name: "Eve".into(), proof, fresh: true })).unwrap();
    assert_eq!(wire::read(&mut r, 0).unwrap(), Msg::Refused(Refusal::Password));
}

// 3. what an edit may add

#[test]
fn edits_beyond_the_hosts_bounds_are_turned_down_whole() {
    let h = Host::start(HostConfig { room_bytes: 1000, room_parts: 5, margin: 2, ..cfg() }, map(100)).unwrap();
    let c = join(&h, PASSWORD).unwrap();
    let edit = |seq: u64, parts: &[(Part, u64, usize)]| Msg::Edit { seq, base: parts.iter().map(|p| (p.0, p.1)).collect(), states: parts.iter().map(|p| State { part: p.0, version: 0, data: Some(vec![2; p.2]) }).collect() };
    let rejected = |m: Msg| match m {
        Msg::Rejected { why, states, .. } => (why, states),
        m => panic!("taken: {m:?}"),
    };

    // a tile far off the map (tiles 0..3, 0 and 2 beyond)
    c.send(edit(1, &[(Part::Tile(1, 0), 1, 10), (Part::Tile(50, 0), 0, 10)]));
    let (why, states) = rejected(next(&c));
    assert_eq!(why, Why::NotThere);
    // (both put back: tile 1, 0 as it is, tile 50, 0 as never there)
    assert_eq!(states.len(), 2);
    // a part the host does not keep
    c.send(edit(2, &[(Part::Timetable, 0, 10)]));
    assert_eq!(rejected(next(&c)).0, Why::NotThere);
    // more bytes than the room left
    c.send(edit(3, &[(Part::Tile(4, 0), 0, 1101)]));
    assert_eq!(rejected(next(&c)).0, Why::TooMuch);
    // more new parts than the room left (5)
    let many: Vec<(Part, u64, usize)> = (-2..=4).map(|x| (Part::Tile(x, 1), 0, 1)).collect();
    c.send(edit(4, &many));
    assert_eq!(rejected(next(&c)).0, Why::TooMuch);
    // within all of it: taken; a tile replaced by a smaller one gives its room back
    c.send(edit(5, &[(Part::Tile(4, 0), 0, 1000)]));
    let Msg::Applied { states, .. } = next(&c) else { panic!() };
    c.send(edit(6, &[(Part::Tile(4, 0), states[0].version, 1)]));
    let Msg::Applied { .. } = next(&c) else { panic!() };
    c.send(edit(7, &[(Part::Tile(-2, 0), 0, 999)]));
    let Msg::Applied { .. } = next(&c) else { panic!() };
}

#[test]
fn a_message_larger_than_a_peer_may_send_ends_the_connection() {
    let h = Host::start(cfg(), map(10)).unwrap();
    let c = join(&h, PASSWORD).unwrap();
    // (64 MiB at most, counted unpacked)
    let data = vec![0u8; 65 << 20];
    c.send(Msg::Edit { seq: 1, base: vec![(Part::Tile(0, 0), 1)], states: vec![State { part: Part::Tile(0, 0), version: 0, data: Some(data) }] });
    until("the host kept the connection", 20, || c.poll().iter().any(|e| matches!(e, Event::Closed(_))));
}

#[test]
fn presence_is_cut_to_size() {
    let h = Host::start(cfg(), map(10)).unwrap();
    let a = join(&h, PASSWORD).unwrap();
    let b = join(&h, PASSWORD).unwrap();
    a.send(Msg::Presence(Presence { cursor: None, selected: (0..5000).map(|i| (0, 0, i)).collect(), tool: "x".repeat(10_000) }));
    let Msg::Seen { presence, .. } = next(&b) else { panic!() };
    assert_eq!((presence.selected.len(), presence.tool.len()), (200, 32));
}

// 4. no password only on purpose

#[test]
fn an_empty_password_is_taken_only_when_meant() {
    assert!(Host::start(HostConfig { password: String::new(), ..cfg() }, map(10)).is_err());
    let h = Host::start(HostConfig { password: String::new(), open: true, ..cfg() }, map(10)).unwrap();
    assert!(join(&h, "").is_ok());
}
