//! sha256 checksum files (`sha256sum` format) and hex helpers.

use crate::{Error, Result};

/// Extracts the digest for `file_name` from a `sha256sum`-style file: lines of `<hex>  <name>` (or `<hex> *<name>`
/// for binary mode). A line with only a hash is accepted as is. Names are compared by their last path component, so
/// `dist/shrew-x` matches `shrew-x`.
pub fn parse_checksum(text: &str, file_name: &str) -> Result<[u8; 32]> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).peekable();
    let mut seen = 0;
    while let Some(line) = lines.next() {
        seen += 1;
        let (hash, name) = match line.split_once(char::is_whitespace) {
            Some((hash, rest)) => (hash, Some(rest.trim_start().trim_start_matches('*'))),
            None => (line, None),
        };
        let matches = match name {
            Some(name) => name.rsplit(['/', '\\']).next() == Some(file_name),
            // A bare hash only makes sense as the whole file.
            None => seen == 1 && lines.peek().is_none(),
        };
        if matches {
            return decode_hex(hash).ok_or_else(|| Error::Feed(format!("bad sha256 for {file_name}: {hash:?}")));
        }
    }
    Err(Error::Feed(format!("checksum file has no entry for {file_name}")))
}

pub(crate) fn decode_hex(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let nibble = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = [0; 32];
    for (i, pair) in bytes.chunks_exact(2).enumerate() {
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
