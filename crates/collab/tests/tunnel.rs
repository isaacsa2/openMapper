//! Joining through the gateway a tunnel carries, with openOMSI's own forwarding on the peer's
//! side (the tunnel itself is left out: it only carries the WebSocket).

use std::time::{Duration, Instant};

use openmapper_collab::gateway::{self, Gateway};
use openmapper_collab::{Client, Event, Host, HostConfig, MapInfo, Msg, Part, State};

#[test]
fn a_peer_joins_and_edits_through_the_gateway() {
    let cfg = HostConfig { bind: "127.0.0.1:0".parse().unwrap(), password: "a long password".into(), work: 10, map: MapInfo { folder: "Testmap".into(), name: "Test".into() }, next_id: 1, max_peers: 8, ..Default::default() };
    // (a tile big enough to need many WebSocket messages)
    let tile: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    let h = Host::start(cfg, vec![State { part: Part::Tile(0, 0), version: 0, data: Some(tile.clone()) }]).unwrap();
    let gw = Gateway::start(h.addr()).unwrap();
    let url = format!("ws://{}{}", gw.addr(), gateway::PATH);
    let local = omsi_net::ws::tcp_forward(&url).unwrap();
    let (far, w) = Client::connect(&local.to_string(), "Far away", "a long password", true).unwrap();
    assert_eq!(w.states[0].data.as_deref(), Some(&tile[..]));
    let (near, _) = Client::connect(&h.addr().to_string(), "Near", "a long password", true).unwrap();
    far.send(Msg::Edit { seq: 1, base: vec![(Part::Tile(0, 0), 1)], states: vec![State { part: Part::Tile(0, 0), version: 0, data: Some(b"edited far away".to_vec()) }] });
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let got = near.poll().into_iter().any(|e| matches!(e, Event::Msg(Msg::Applied { states, .. }) if states[0].data.as_deref() == Some(&b"edited far away"[..])));
        if got {
            break;
        }
        assert!(Instant::now() < until, "the edit never came");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_gateway_opens_its_path_only() {
    let gw = Gateway::start("127.0.0.1:9".parse().unwrap()).unwrap();
    let r = tungstenite::connect(format!("ws://{}/somewhere", gw.addr()));
    assert!(r.is_err());
}
