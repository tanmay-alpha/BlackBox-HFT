//! Canonical JSON bytes for digests over `serde_json::Value`: object keys sorted, no whitespace.
//! Independent of serde_json's `preserve_order` feature, so a value read back from jsonb hashes
//! the same as the value that was written.

use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn to_vec(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

pub fn sha256(value: &Value) -> [u8; 32] {
    Sha256::digest(to_vec(value)).into()
}

fn write(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(
                    serde_json::to_string(key)
                        .expect("a string always serializes")
                        .as_bytes(),
                );
                out.push(b':');
                write(&map[key], out);
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write(item, out);
            }
            out.push(b']');
        }
        scalar => out.extend_from_slice(scalar.to_string().as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_does_not_change_the_digest() {
        let a = json!({"b": 1, "a": {"y": [1, 2], "x": "s"}});
        let b: Value = serde_json::from_str(r#"{"a":{"x":"s","y":[1,2]},"b":1}"#).unwrap();
        assert_eq!(to_vec(&a), br#"{"a":{"x":"s","y":[1,2]},"b":1}"#);
        assert_eq!(sha256(&a), sha256(&b));
    }
}
