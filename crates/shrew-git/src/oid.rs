//! Object ids, independent of gix's types so the UI crates never name gix.

use std::fmt;

/// A SHA-1 object id (commit, tree or blob).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Oid([u8; 20]);

impl Oid {
    pub const ZERO: Oid = Oid([0; 20]);

    pub fn from_bytes(bytes: [u8; 20]) -> Oid {
        Oid(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Parses 40 hex digits.
    pub fn from_hex(hex: &str) -> Option<Oid> {
        let hex = hex.as_bytes();
        if hex.len() != 40 {
            return None;
        }
        let mut out = [0u8; 20];
        for (i, pair) in hex.chunks(2).enumerate() {
            let digit = |c: u8| (c as char).to_digit(16);
            out[i] = (digit(pair[0])? * 16 + digit(pair[1])?) as u8;
        }
        Some(Oid(out))
    }

    /// The first 7 hex digits, as `git log --oneline` shows them.
    pub fn short(&self) -> String {
        let mut s = self.to_string();
        s.truncate(7);
        s
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0; 20]
    }

    pub(crate) fn from_gix(id: &gix::oid) -> Oid {
        let mut out = [0u8; 20];
        let bytes = id.as_bytes();
        let n = bytes.len().min(20);
        out[..n].copy_from_slice(&bytes[..n]);
        Oid(out)
    }

    pub(crate) fn to_gix(self) -> gix::ObjectId {
        gix::ObjectId::from_bytes_or_panic(&self.0)
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.short())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let id = Oid::from_hex(hex).unwrap();
        assert_eq!(id.to_string(), hex);
        assert_eq!(id.short(), "0123456");
        assert!(Oid::from_hex("xyz").is_none());
        assert!(Oid::from_hex(&hex[..39]).is_none());
        assert_eq!(Oid::from_gix(&id.to_gix()), id);
    }
}
