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
/// The encoding is streamed through a 64 KiB buffer into the hasher, never held whole: a run's
/// tape, fills and equity encode to well over 100 MB.
pub fn sha256_json<T: Serialize>(value: &T) -> String {
    let mut writer = io::BufWriter::with_capacity(1 << 16, HashWriter(Sha256::new()));
    serde_json::to_writer(&mut writer, value).expect("contract types serialize infallibly");
    let HashWriter(hasher) = writer
        .into_inner()
        .unwrap_or_else(|_| unreachable!("hashing never fails"));
    hex::encode(hasher.finalize())
}

struct HashWriter(Sha256);
impl io::Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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

    #[test]
    fn streamed_json_digest_equals_the_buffered_encoding() {
        use std::collections::{BTreeMap, HashMap};
        let buffered = |bytes: Vec<u8>| sha256_hex(&bytes);
        let nested = serde_json::json!({
            "z": {"b": [0.1, 1e-7, -0.0, 1.7976931348623157e308, 5e-324], "a": null},
            "a": [{"y": "\u{e9}\"\\\n\t", "x": u64::MAX, "w": i64::MIN}, true, false, []],
            "m": {}
        });
        assert_eq!(
            sha256_json(&nested),
            buffered(serde_json::to_vec(&nested).unwrap())
        );
        let floats: Vec<f64> = vec![
            0.1,
            -0.0,
            f64::NAN,
            f64::INFINITY,
            2.5e-3,
            123_456_789.123_456_79,
        ];
        assert_eq!(
            sha256_json(&floats),
            buffered(serde_json::to_vec(&floats).unwrap())
        );
        let map: BTreeMap<String, Vec<Option<f64>>> = (0..50)
            .map(|i| (format!("k{i}"), vec![Some(i as f64 / 7.0), None]))
            .collect();
        let single: HashMap<&str, (u8, &str)> = HashMap::from([("only", (1, "x"))]);
        assert_eq!(
            sha256_json(&(&map, &single)),
            buffered(serde_json::to_vec(&(&map, &single)).unwrap())
        );
        // Larger than any internal buffer: many write calls of varied sizes.
        let large: Vec<(u64, f64, String)> = (0..200_000u64)
            .map(|i| (i, i as f64 * 0.37, "r".repeat((i % 13) as usize)))
            .collect();
        assert_eq!(
            sha256_json(&large),
            buffered(serde_json::to_vec(&large).unwrap())
        );
    }
}
