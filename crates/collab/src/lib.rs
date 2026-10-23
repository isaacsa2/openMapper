//! Editing one map together: one openMapper hosts it, the others join over TCP.
//!
//! The map is cut into parts (each tile, the global settings, the timetable, the traffic, the
//! descriptions, the groups), each kept by the host as bytes with a version. An editor sends
//! the parts an edit changed together with the versions it changed them from; the host takes
//! the edit only when none of them changed in the meantime, gives the parts new versions and
//! passes them on to everybody. An edit made on a part somebody else changed first is turned
//! down, and its sender gets the part as it is now: nobody's work is written over unseen.
//!
//! What the bytes of a part are is the editor's business (this crate never reads them). The
//! host's own editor joins its host like any other, over the loopback.

pub mod auth;
pub mod client;
pub mod gateway;
pub mod host;
pub mod wire;

pub use client::{Client, Event, JoinError, Welcome};
pub use host::{Host, HostConfig};
pub use wire::{MapInfo, Msg, Part, Peer, Presence, Refusal, State, PROTOCOL};

/// The port a host listens on when none is given.
pub const DEFAULT_PORT: u16 = 27_560;

/// How many IDCodes a peer is given at a time (asked again when they run low).
pub const ID_BLOCK: i64 = 10_000;
