//! Content hashing (BLAKE3): bytes, strings, hex encoding, and an order-sensitive combine over parts.

pub fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

pub fn hash_str(s: &str) -> [u8; 32] {
    hash_bytes(s.as_bytes())
}

pub fn to_hex(h: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in h {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Order-sensitive hash over a sequence of byte parts, with a length prefix per
/// part so `["ab","c"]` and `["a","bc"]` hash differently.
pub fn combine(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_hex_is_64_chars() {
        let h = hash_bytes(b"hello");
        assert_eq!(to_hex(&h).len(), 64);
        // BLAKE3("hello") known vector.
        assert_eq!(
            to_hex(&h),
            "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f"
        );
    }

    #[test]
    fn combine_is_order_sensitive() {
        let a = combine(&[b"x", b"y"]);
        let b = combine(&[b"y", b"x"]);
        assert_ne!(a, b);
    }
}
