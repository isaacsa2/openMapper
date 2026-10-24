//! The messages and how they go over the stream: `OMMP`, a JSON header, then the bytes of
//! the parts the message carries, each deflated.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// The protocol's version: host and peer must have the same.
pub const PROTOCOL: u32 = 1;

const MAGIC: &[u8; 4] = b"OMMP";
/// The largest header read (the parts' bytes come after it).
const MAX_HEADER: usize = 4 << 20;

/// A piece of the map that is edited and passed on as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Part {
    Tile(i32, i32),
    Global,
    Timetable,
    Traffic,
    Descriptions,
    Groups,
}

/// A part as it is: its version and its bytes (None: not there, a tile taken away).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub part: Part,
    pub version: u64,
    /// (in the header only whether there are bytes; they follow it)
    #[serde(with = "present")]
    pub data: Option<Vec<u8>>,
}

mod present {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(d.is_some())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Ok(bool::deserialize(d)?.then(Vec::new))
    }
}

/// Somebody editing the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub id: u32,
    pub name: String,
    /// The colour their cursor and selection are drawn in.
    pub color: [u8; 3],
}

/// The map a host shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapInfo {
    /// Its folder name (under `maps`).
    pub folder: String,
    /// Its friendly name.
    pub name: String,
}

/// Where somebody is working, passed on to the others (not kept).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Presence {
    /// The cursor on the ground (world metres).
    pub cursor: Option<[f64; 3]>,
    /// The tile and IDCode of each selected thing (object or spline).
    pub selected: Vec<(i32, i32, i64)>,
    /// The tool in hand, as the editor names it.
    pub tool: String,
}

/// Why a host turned an edit down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Why {
    /// One of its parts was changed by somebody else first.
    Stale,
    /// The host would hold more than it takes (bytes or parts), or the edit is too big.
    TooMuch,
    /// A tile far outside the map, or a part the host does not keep.
    NotThere,
}

/// Why a host turned somebody away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// Another version of the protocol (the host's).
    Protocol(u32),
    Password,
    Full,
    /// The name is empty or too long.
    Name,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    // host → peer
    /// The first message: the host's protocol, the nonce the password is proved with, and the
    /// salt and work factor its key is derived with (see `auth`).
    Challenge { protocol: u32, nonce: String, salt: String, work: u32 },
    Welcome { you: Peer, map: MapInfo, ids: (i64, i64), peers: Vec<Peer>, states: Vec<State> },
    Refused(Refusal),
    /// An edit taken: the parts with their new versions (`seq`: the author's, for the author).
    Applied { author: u32, seq: Option<u64>, states: Vec<State> },
    /// An edit turned down (one of its parts changed meanwhile): each part it changed, as it
    /// is now.
    Rejected { seq: u64, why: Why, states: Vec<State> },
    Peers(Vec<Peer>),
    Seen { peer: u32, presence: Presence },
    Said { peer: u32, text: String },
    /// A new block of IDCodes (from, to: `to` not included).
    Ids(i64, i64),

    // peer → host
    /// `fresh`: send the map's parts (the host's own editor has them already).
    Hello { protocol: u32, name: String, proof: String, fresh: bool },
    /// An edit: each part's new state, and the version each was changed from (0: never there).
    Edit { seq: u64, base: Vec<(Part, u64)>, states: Vec<State> },
    Presence(Presence),
    Say(String),
    NeedIds,

    /// Both ways, to know the other is still there.
    Ping,
}

impl Msg {
    fn states(&self) -> Option<&Vec<State>> {
        match self {
            Msg::Welcome { states, .. } | Msg::Applied { states, .. } | Msg::Rejected { states, .. } | Msg::Edit { states, .. } => Some(states),
            _ => None,
        }
    }

    fn states_mut(&mut self) -> Option<&mut Vec<State>> {
        match self {
            Msg::Welcome { states, .. } | Msg::Applied { states, .. } | Msg::Rejected { states, .. } | Msg::Edit { states, .. } => Some(states),
            _ => None,
        }
    }
}

/// A message as it goes over the stream.
pub fn encode(m: &Msg) -> Vec<u8> {
    let head = serde_json::to_vec(m).expect("a message is always written");
    let mut out = Vec::with_capacity(head.len() + 16);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    for d in m.states().into_iter().flatten().filter_map(|s| s.data.as_ref()) {
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(d).expect("writing to memory");
        let z = z.finish().expect("writing to memory");
        out.extend_from_slice(&(z.len() as u32).to_le_bytes());
        out.extend_from_slice(&z);
    }
    out
}

/// Read one message; `limit`: the most bytes its parts may come to (unpacked).
pub fn read(r: &mut impl Read, limit: u64) -> io::Result<Msg> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad("not an openMapper stream"));
    }
    let n = read_u32(r)? as usize;
    if n > MAX_HEADER {
        return Err(bad("header too large"));
    }
    let mut head = vec![0u8; n];
    r.read_exact(&mut head)?;
    let mut m: Msg = serde_json::from_slice(&head).map_err(|e| bad(&format!("unreadable message: {e}")))?;
    let mut left = limit;
    for s in m.states_mut().into_iter().flatten() {
        let Some(d) = &mut s.data else { continue };
        let z = read_u32(r)? as u64;
        // (unpacked no further than what is left: a small packed part cannot blow up)
        let mut inflate = flate2::read::DeflateDecoder::new(r.by_ref().take(z)).take(left + 1);
        inflate.read_to_end(d)?;
        if d.len() as u64 > left {
            return Err(bad("message too large"));
        }
        left -= d.len() as u64;
        // (whatever the deflate stream left of its packed bytes)
        io::copy(&mut inflate.into_inner().into_inner(), &mut io::sink())?;
    }
    Ok(m)
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_with_parts_reads_back() {
        let m = Msg::Edit {
            seq: 3,
            base: vec![(Part::Tile(0, -1), 4), (Part::Global, 0)],
            states: vec![State { part: Part::Tile(0, -1), version: 0, data: Some(vec![7; 100_000]) }, State { part: Part::Global, version: 0, data: None }, State { part: Part::Groups, version: 0, data: Some(Vec::new()) }],
        };
        let bytes = encode(&m);
        // (deflated: the repeated bytes take little room)
        assert!(bytes.len() < 2_000);
        let back = read(&mut bytes.as_slice(), 1 << 20).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn messages_follow_each_other() {
        let a = Msg::Say("<b>hi</b>; kick 3".into());
        let b = Msg::Applied { author: 2, seq: None, states: vec![State { part: Part::Timetable, version: 9, data: Some(b"x".to_vec()) }] };
        let mut bytes = encode(&a);
        bytes.extend(encode(&b));
        let mut r = bytes.as_slice();
        assert_eq!(read(&mut r, 1 << 20).unwrap(), a);
        assert_eq!(read(&mut r, 1 << 20).unwrap(), b);
        assert!(r.is_empty());
    }

    #[test]
    fn parts_larger_than_the_limit_are_refused() {
        let m = Msg::Edit { seq: 1, base: vec![], states: vec![State { part: Part::Global, version: 0, data: Some(vec![0; 10_000]) }] };
        let bytes = encode(&m);
        assert!(read(&mut bytes.as_slice(), 9_999).is_err());
        assert!(read(&mut bytes.as_slice(), 10_000).is_ok());
    }

    #[test]
    fn something_else_is_not_read() {
        assert!(read(&mut &b"GET / HTTP/1.1\r\n\r\n"[..], 1 << 20).is_err());
        let mut bytes = encode(&Msg::Ping);
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&mut bytes.as_slice(), 1 << 20).is_err());
    }
}
