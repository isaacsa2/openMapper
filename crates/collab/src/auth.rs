//! The password: the host sends a fresh nonce, the peer answers with a hash of it and the
//! password, so the password itself never goes over the stream.
//!
//! The stream is not encrypted: somebody who can listen in sees the map and could try
//! passwords against a nonce and its answer. Over the internet a host goes behind a VPN or a
//! tunnel, and a password is a long one.

use sha2::{Digest, Sha256};

/// The answer to `nonce` for `password`.
pub fn proof(nonce: &str, password: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"openMapper multiplayer\0");
    h.update(nonce.as_bytes());
    h.update([0]);
    h.update(password.as_bytes());
    hex(&h.finalize())
}

/// Whether `answer` proves `password` for `nonce` (compared in constant time).
pub fn check(nonce: &str, password: &str, answer: &str) -> bool {
    let want = proof(nonce, password);
    want.len() == answer.len() && want.bytes().zip(answer.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

/// A nonce nobody can guess ahead: the system's hasher keys (random per process), the time
/// and a counter, hashed.
pub fn nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut h = Sha256::new();
    for _ in 0..4 {
        let mut s = std::collections::hash_map::RandomState::new().build_hasher();
        s.write_u64(COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        h.update(s.finish().to_le_bytes());
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    h.update(t.as_nanos().to_le_bytes());
    hex(&h.finalize()[..16])
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_right_password_is_taken_only() {
        let n = nonce();
        assert!(check(&n, "bus stop", &proof(&n, "bus stop")));
        assert!(!check(&n, "bus stop", &proof(&n, "bus stoP")));
        assert!(!check(&n, "bus stop", &proof(&nonce(), "bus stop")));
        assert!(!check(&n, "bus stop", ""));
    }

    #[test]
    fn nonces_differ() {
        let a: std::collections::HashSet<String> = (0..100).map(|_| nonce()).collect();
        assert_eq!(a.len(), 100);
    }
}
