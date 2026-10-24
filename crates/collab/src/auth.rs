//! The password: the host sends a fresh nonce with a salt and a work factor; the peer derives
//! a key from the password with PBKDF2-HMAC-SHA256 (salt, work) and answers with an HMAC of
//! the nonce under that key. The password never goes over the stream.
//!
//! The host derives its key once, when it starts: checking an answer costs it one HMAC, so
//! answers sent in bulk cost it little. Somebody who listens in on a LAN connection (it is not
//! encrypted) and keeps a nonce and its answer must pay the whole derivation for every
//! password they try: that slows guessing down, it does not stop it for a short password.

use sha2::{Digest, Sha256};

/// The work factor a host asks for (PBKDF2 rounds): about a tenth of a second on a PC.
pub const WORK: u32 = 200_000;
/// The most a peer derives for (a host asking for more is not one to wait for).
pub const MAX_WORK: u32 = 5_000_000;

/// The key a password gives under a salt and a work factor.
#[derive(Clone)]
pub struct Key([u8; 32]);

/// PBKDF2-HMAC-SHA256, one block (32 bytes).
pub fn derive(password: &str, salt: &str, work: u32) -> Key {
    let mut first = salt.as_bytes().to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(password.as_bytes(), &first);
    let mut out = u;
    for _ in 1..work.max(1) {
        u = hmac(password.as_bytes(), &u);
        out.iter_mut().zip(u).for_each(|(o, x)| *o ^= x);
    }
    Key(out)
}

/// The answer to `nonce` under `key`.
pub fn proof(key: &Key, nonce: &str) -> String {
    let mut m = b"openMapper multiplayer\0".to_vec();
    m.extend_from_slice(nonce.as_bytes());
    hex(&hmac(&key.0, &m))
}

/// Whether `answer` is the answer to `nonce` under `key` (compared in constant time).
pub fn check(key: &Key, nonce: &str, answer: &str) -> bool {
    let want = proof(key, nonce);
    want.len() == answer.len() && want.bytes().zip(answer.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

/// HMAC-SHA256.
fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |x: u8| k.iter().map(|b| b ^ x).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

/// A nonce (or salt) nobody can guess ahead: the system's hasher keys (random per process),
/// the time and a counter, hashed.
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
        let (salt, n) = (nonce(), nonce());
        let key = derive("bus stop", &salt, 10);
        assert!(check(&key, &n, &proof(&derive("bus stop", &salt, 10), &n)));
        assert!(!check(&key, &n, &proof(&derive("bus stoP", &salt, 10), &n)));
        assert!(!check(&key, &n, &proof(&derive("bus stop", &nonce(), 10), &n)));
        assert!(!check(&key, &n, &proof(&derive("bus stop", &salt, 11), &n)));
        assert!(!check(&key, &n, &proof(&key, &nonce())));
        assert!(!check(&key, &n, ""));
    }

    /// Against the published test vectors.
    #[test]
    fn hmac_and_pbkdf2_are_the_standard_ones() {
        // RFC 4231, test case 2
        assert_eq!(hex(&hmac(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // RFC 7914 section 11 (PBKDF2-HMAC-SHA256, "passwd", "salt", 1 round), first 32 bytes
        assert_eq!(hex(&derive("passwd", "salt", 1).0), "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc");
        // and 4096 rounds of "password", "salt" (the widely published vector)
        assert_eq!(hex(&derive("password", "salt", 4096).0), "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a");
    }

    #[test]
    fn nonces_differ() {
        let a: std::collections::HashSet<String> = (0..100).map(|_| nonce()).collect();
        assert_eq!(a.len(), 100);
    }
}
