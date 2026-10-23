//! Hosts and peers over the loopback, as two editors would use them.

use std::time::{Duration, Instant};

use openmapper_collab::client::Event;
use openmapper_collab::{Client, Host, HostConfig, JoinError, MapInfo, Msg, Part, Presence, Refusal, State, ID_BLOCK};

fn host(password: &str, max_peers: usize) -> Host {
    let cfg = HostConfig { bind: "127.0.0.1:0".parse().unwrap(), password: password.into(), map: MapInfo { folder: "Testmap".into(), name: "Test map".into() }, next_id: 500, max_peers };
    let states = vec![State { part: Part::Tile(0, 0), version: 0, data: Some(b"tile 0 0".to_vec()) }, State { part: Part::Global, version: 0, data: Some(b"global".to_vec()) }];
    Host::start(cfg, states).unwrap()
}

fn join(h: &Host, name: &str, password: &str) -> Result<(Client, openmapper_collab::Welcome), JoinError> {
    Client::connect(&h.addr().to_string(), name, password, true)
}

/// The next message (other than the peer lists) within a few seconds.
fn next(c: &Client) -> Msg {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        for e in c.poll() {
            match e {
                Event::Msg(Msg::Peers(_)) => {}
                Event::Msg(m) => return m,
                Event::Closed(why) => panic!("closed: {why}"),
            }
        }
        assert!(Instant::now() < until, "nothing came");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn edit(seq: u64, part: Part, from: u64, data: &[u8]) -> Msg {
    Msg::Edit { seq, base: vec![(part, from)], states: vec![State { part, version: 0, data: Some(data.to_vec()) }] }
}

#[test]
fn a_peer_gets_the_map_and_the_others_edits() {
    let h = host("", 8);
    let (a, wa) = join(&h, "Ana", "").unwrap();
    let (b, wb) = join(&h, "Bruno", "").unwrap();
    assert_eq!(wa.map.folder, "Testmap");
    assert_eq!(wb.states.iter().find(|s| s.part == Part::Tile(0, 0)).unwrap().data.as_deref(), Some(&b"tile 0 0"[..]));
    assert_eq!(wb.peers.len(), 2);
    assert_ne!(wa.you.color, wb.you.color);

    a.send(edit(1, Part::Tile(0, 0), 1, b"tile 0 0 with a bus stop"));
    let Msg::Applied { seq: Some(1), states, .. } = next(&a) else { panic!() };
    // (the author is told the new version, without its own bytes again)
    assert_eq!((states[0].part, states[0].data.is_none()), (Part::Tile(0, 0), true));
    let v = states[0].version;
    let Msg::Applied { author, seq: None, states } = next(&b) else { panic!() };
    assert_eq!(author, wa.you.id);
    assert_eq!((states[0].version, states[0].data.as_deref()), (v, Some(&b"tile 0 0 with a bus stop"[..])));
}

#[test]
fn an_edit_on_a_part_changed_meanwhile_is_turned_down() {
    let h = host("", 8);
    let (a, _) = join(&h, "Ana", "").unwrap();
    let (b, _) = join(&h, "Bruno", "").unwrap();
    a.send(edit(1, Part::Tile(0, 0), 1, b"Ana's"));
    let Msg::Applied { .. } = next(&a) else { panic!() };
    // Bruno edited the tile as it was before Ana's edit came to him
    b.send(edit(7, Part::Tile(0, 0), 1, b"Bruno's"));
    let Msg::Applied { seq: None, .. } = next(&b) else { panic!() };
    let Msg::Rejected { seq: 7, states } = next(&b) else { panic!() };
    assert_eq!(states[0].data.as_deref(), Some(&b"Ana's"[..]));
    // and Ana never sees Bruno's
    std::thread::sleep(Duration::from_millis(100));
    assert!(a.poll().iter().all(|e| matches!(e, Event::Msg(Msg::Peers(_)))));
}

#[test]
fn an_edit_is_taken_whole_or_not_at_all() {
    let h = host("", 8);
    let (a, _) = join(&h, "Ana", "").unwrap();
    // a road over two tiles: one of them is new (never there: version 0), the other stale
    let m = Msg::Edit { seq: 2, base: vec![(Part::Tile(1, 0), 0), (Part::Tile(0, 0), 99)], states: vec![State { part: Part::Tile(1, 0), version: 0, data: Some(b"new".to_vec()) }, State { part: Part::Tile(0, 0), version: 0, data: Some(b"x".to_vec()) }] };
    a.send(m);
    let Msg::Rejected { seq: 2, states } = next(&a) else { panic!() };
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].part, Part::Tile(0, 0));
    // the new tile was not added: a peer joining now does not get it
    let (_, w) = join(&h, "Bruno", "").unwrap();
    assert!(w.states.iter().all(|s| s.part != Part::Tile(1, 0)));
}

#[test]
fn a_tile_taken_away_stays_known_as_gone() {
    let h = host("", 8);
    let (a, _) = join(&h, "Ana", "").unwrap();
    a.send(Msg::Edit { seq: 1, base: vec![(Part::Tile(0, 0), 1)], states: vec![State { part: Part::Tile(0, 0), version: 0, data: None }] });
    let Msg::Applied { states, .. } = next(&a) else { panic!() };
    let gone = states[0].version;
    let (_, w) = join(&h, "Bruno", "").unwrap();
    let s = w.states.iter().find(|s| s.part == Part::Tile(0, 0)).unwrap();
    // (with its version: adding it again is made from that one)
    assert_eq!((s.version, s.data.is_none()), (gone, true));
}

#[test]
fn the_password_is_needed() {
    let h = host("long enough password", 8);
    assert!(matches!(join(&h, "Ana", "wrong"), Err(JoinError::Refused(Refusal::Password))));
    assert!(matches!(join(&h, "Ana", ""), Err(JoinError::Refused(Refusal::Password))));
    assert!(join(&h, "Ana", "long enough password").is_ok());
}

#[test]
fn names_are_cleaned_and_checked() {
    let h = host("", 8);
    assert!(matches!(join(&h, "  \n ", ""), Err(JoinError::Refused(Refusal::Name))));
    assert!(matches!(join(&h, &"x".repeat(33), ""), Err(JoinError::Refused(Refusal::Name))));
    let (_, w) = join(&h, " Ana\u{7}\r\n", "").unwrap();
    assert_eq!(w.you.name, "Ana");
}

#[test]
fn a_full_host_turns_away() {
    let h = host("", 2);
    let (_a, _) = join(&h, "Ana", "").unwrap();
    let (_b, _) = join(&h, "Bruno", "").unwrap();
    assert!(matches!(join(&h, "Carla", ""), Err(JoinError::Refused(Refusal::Full))));
}

#[test]
fn every_peer_has_its_own_idcodes() {
    let h = host("", 8);
    let (a, wa) = join(&h, "Ana", "").unwrap();
    let (_b, wb) = join(&h, "Bruno", "").unwrap();
    assert_eq!(wa.ids, (500, 500 + ID_BLOCK));
    assert_eq!(wb.ids, (500 + ID_BLOCK, 500 + 2 * ID_BLOCK));
    a.send(Msg::NeedIds);
    let Msg::Ids(x, y) = next(&a) else { panic!() };
    assert_eq!((x, y), (500 + 2 * ID_BLOCK, 500 + 3 * ID_BLOCK));
}

#[test]
fn presence_and_chat_go_to_the_others() {
    let h = host("", 8);
    let (a, wa) = join(&h, "Ana", "").unwrap();
    let (b, _) = join(&h, "Bruno", "").unwrap();
    a.send(Msg::Presence(Presence { cursor: Some([1.0, 2.0, 3.0]), selected: vec![(0, 0, 9)], tool: "Roads".into() }));
    let Msg::Seen { peer, presence } = next(&b) else { panic!() };
    assert_eq!((peer, presence.cursor), (wa.you.id, Some([1.0, 2.0, 3.0])));
    a.send(Msg::Say("<script>x</script>\u{1b}[31m".into()));
    // (passed on as text, control characters left out; the editor shows it as text)
    let Msg::Said { text, .. } = next(&b) else { panic!() };
    assert_eq!(text, "<script>x</script>[31m");
}

#[test]
fn a_peer_leaving_is_seen_by_the_others() {
    let h = host("", 8);
    let (a, _) = join(&h, "Ana", "").unwrap();
    let (b, _) = join(&h, "Bruno", "").unwrap();
    drop(b);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if a.poll().iter().any(|e| matches!(e, Event::Msg(Msg::Peers(p)) if p.len() == 1)) {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_peers_see_the_host_stop() {
    let h = host("", 8);
    let (a, _) = join(&h, "Ana", "").unwrap();
    drop(h);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if a.poll().iter().any(|e| matches!(e, Event::Closed(_))) {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn something_that_is_not_a_host_is_told_apart() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::Write;
        let (mut s, _) = l.accept().unwrap();
        let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
    });
    assert!(matches!(Client::connect(&addr.to_string(), "Ana", "", true), Err(JoinError::NotAHost)));
}

#[test]
fn a_peer_that_does_not_read_is_let_go() {
    use std::io::Write;
    let h = host("", 8);
    let (a, _) = join(&h, "Ana", "").unwrap();
    // a peer that says hello and then reads nothing more
    let mut s = std::net::TcpStream::connect(h.addr()).unwrap();
    let mut r = std::io::BufReader::new(s.try_clone().unwrap());
    let Msg::Challenge { nonce, .. } = openmapper_collab::wire::read(&mut r, 0).unwrap() else { panic!() };
    let hello = Msg::Hello { protocol: openmapper_collab::PROTOCOL, name: "Slow".into(), proof: openmapper_collab::auth::proof(&nonce, ""), fresh: true };
    s.write_all(&openmapper_collab::wire::encode(&hello)).unwrap();
    // Ana's edits keep coming (bytes that do not pack, so the slow peer's socket fills); the
    // host must not wait for the slow one, it lets it go
    let mut version = 1;
    let mut seed = 1u32;
    let mut slow_seen = false;
    let until = Instant::now() + Duration::from_secs(60);
    for seq in 1.. {
        let data: Vec<u8> = (0..1 << 18)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        a.send(edit(seq, Part::Tile(0, 0), version, &data));
        let applied = Instant::now() + Duration::from_secs(5);
        let mut got = false;
        while !got {
            for e in a.poll() {
                match e {
                    Event::Msg(Msg::Applied { states, .. }) => {
                        version = states[0].version;
                        got = true;
                    }
                    Event::Msg(Msg::Peers(p)) => {
                        let slow = p.iter().any(|p| p.name == "Slow");
                        if slow_seen && !slow {
                            return;
                        }
                        slow_seen |= slow;
                    }
                    Event::Msg(_) => {}
                    Event::Closed(why) => panic!("closed: {why}"),
                }
            }
            assert!(Instant::now() < applied, "the host stopped taking edits");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(Instant::now() < until, "the slow peer was never let go");
    }
}
