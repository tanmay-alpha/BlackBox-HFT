//! Content hashing for manifests and results.

use std::io::{self, Read};

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Lower-case hex SHA-256 of everything `reader` yields.
pub fn sha256_reader<R: Read>(mut reader: R) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA-256 of the canonical JSON encoding of `value`.
///
/// Canonical here means `serde_json::to_vec` of the Rust value: struct fields serialize in
/// declaration order and no maps with unordered keys are used in hashed types, so the encoding is
/// a pure function of the value. Floats use serde_json's shortest round-trip representation.
pub fn sha256_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("contract types serialize infallibly");
    sha256_hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_reader(&b"abc"[..]).unwrap(), sha256_hex(b"abc"));
    }
}
