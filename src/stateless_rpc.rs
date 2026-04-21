//! Git protocol v2 pkt-line encoders + minimal ref-advertisement parser.
//!
//! See `gitprotocol-v2(5)` / `gitprotocol-pack(5)` for the wire format.
//! We encode from scratch (rather than shelling out to `git` on the client
//! side) so the remote-helper's `list` command can return refs without
//! depending on an already-present local clone.

use anyhow::{anyhow, bail, Result};

const FLUSH_PKT: &[u8] = b"0000";
const DELIM_PKT: &[u8] = b"0001";

/// Encode a single pkt-line with the given payload.
/// The payload must be <= 65516 bytes (v2 hard limit).
fn encode_pkt_line(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() + 4;
    assert!(len <= 0xFFFF, "pkt-line payload too large");
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(format!("{:04x}", len).as_bytes());
    out.extend_from_slice(payload);
    out
}

/// Build a v2 `ls-refs` command request.
///
/// This is what a client sends to a stateless-rpc server to learn the
/// refs — the equivalent of the classic ref advertisement.
pub fn encode_ls_refs_request() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&encode_pkt_line(b"command=ls-refs\n"));
    out.extend_from_slice(&encode_pkt_line(b"agent=git-remote-mesh/0.1.0\n"));
    out.extend_from_slice(&encode_pkt_line(b"object-format=sha1\n"));
    out.extend_from_slice(DELIM_PKT);
    out.extend_from_slice(&encode_pkt_line(b"peel\n"));
    out.extend_from_slice(&encode_pkt_line(b"symrefs\n"));
    out.extend_from_slice(&encode_pkt_line(b"ref-prefix HEAD\n"));
    out.extend_from_slice(&encode_pkt_line(b"ref-prefix refs/heads/\n"));
    out.extend_from_slice(&encode_pkt_line(b"ref-prefix refs/tags/\n"));
    out.extend_from_slice(FLUSH_PKT);
    out
}

/// Build a v2 `fetch` command request for the given want shas.
///
/// The server's response will be a pack stream (plus optional
/// sideband progress and acknowledgements). We ask for `no-progress`
/// to keep the reply lean and `ofs-delta`/`thin-pack` for compactness.
pub fn encode_fetch_request(wants: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&encode_pkt_line(b"command=fetch\n"));
    out.extend_from_slice(&encode_pkt_line(b"agent=git-remote-mesh/0.1.0\n"));
    out.extend_from_slice(&encode_pkt_line(b"object-format=sha1\n"));
    out.extend_from_slice(DELIM_PKT);
    out.extend_from_slice(&encode_pkt_line(b"no-progress\n"));
    out.extend_from_slice(&encode_pkt_line(b"ofs-delta\n"));
    out.extend_from_slice(&encode_pkt_line(b"thin-pack\n"));
    for sha in wants {
        let line = format!("want {}\n", sha);
        out.extend_from_slice(&encode_pkt_line(line.as_bytes()));
    }
    out.extend_from_slice(&encode_pkt_line(b"done\n"));
    out.extend_from_slice(FLUSH_PKT);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEntry {
    pub sha: String,
    pub name: String,
    /// Present when ls-refs reports `symref-target:` for a symbolic ref
    /// (typically HEAD).
    pub symref_target: Option<String>,
}

/// Parse the pkt-line-framed response of a v2 `ls-refs` call.
///
/// Each non-flush line is `<sha> <refname>[<space><attr>:<value>...]`.
/// We surface the sha + name pair, and capture `symref-target:` if
/// present so the caller can reproduce `HEAD -> refs/heads/<branch>`.
pub fn parse_refs_advertisement(bytes: &[u8]) -> Result<Vec<RefEntry>> {
    let mut cursor = 0;
    let mut refs = Vec::new();
    while cursor < bytes.len() {
        let (payload, next) = read_pkt_line(bytes, cursor)?;
        cursor = next;
        let payload = match payload {
            PktLine::Flush | PktLine::Delim => continue,
            PktLine::Data(p) => p,
        };
        // Trim trailing newline if any.
        let trimmed = strip_trailing_newline(payload);
        if trimmed.is_empty() {
            continue;
        }
        let text = std::str::from_utf8(trimmed).map_err(|e| anyhow!("non-utf8 ref line: {e}"))?;
        let mut parts = text.splitn(2, ' ');
        let sha = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        if sha.len() != 40 && sha.len() != 64 {
            // Not a ref line (e.g. a capability we didn't recognize). Skip.
            continue;
        }
        let mut attrs = rest.split(' ');
        let name = attrs.next().unwrap_or("").to_string();
        let mut symref_target = None;
        for a in attrs {
            if let Some(t) = a.strip_prefix("symref-target:") {
                symref_target = Some(t.to_string());
            }
        }
        if !name.is_empty() {
            refs.push(RefEntry {
                sha: sha.to_string(),
                name,
                symref_target,
            });
        }
    }
    Ok(refs)
}

enum PktLine<'a> {
    Flush,
    Delim,
    Data(&'a [u8]),
}

fn read_pkt_line(buf: &[u8], pos: usize) -> Result<(PktLine<'_>, usize)> {
    if buf.len() < pos + 4 {
        bail!("truncated pkt-line length prefix at {}", pos);
    }
    let len_str = std::str::from_utf8(&buf[pos..pos + 4])
        .map_err(|e| anyhow!("non-ascii pkt-line length: {e}"))?;
    let len =
        u16::from_str_radix(len_str, 16).map_err(|e| anyhow!("bad hex length {len_str:?}: {e}"))?;
    if len == 0 {
        return Ok((PktLine::Flush, pos + 4));
    }
    if len == 1 {
        return Ok((PktLine::Delim, pos + 4));
    }
    if (len as usize) < 4 {
        bail!("invalid pkt-line length {len} at {}", pos);
    }
    let data_len = (len as usize) - 4;
    if buf.len() < pos + 4 + data_len {
        bail!(
            "truncated pkt-line body at {} (need {} bytes)",
            pos,
            data_len
        );
    }
    Ok((
        PktLine::Data(&buf[pos + 4..pos + 4 + data_len]),
        pos + 4 + data_len,
    ))
}

fn strip_trailing_newline(b: &[u8]) -> &[u8] {
    match b.last() {
        Some(&b'\n') => &b[..b.len() - 1],
        _ => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_refs_request_framing() {
        let enc = encode_ls_refs_request();
        // The first line must declare command=ls-refs.
        assert!(
            enc.starts_with(b"0014command=ls-refs\n"),
            "unexpected prefix: {:?}",
            &enc[..20.min(enc.len())]
        );
        // Must terminate with a flush packet.
        assert!(enc.ends_with(b"0000"), "missing flush-pkt");
        // Must contain the delim separating command from args.
        assert!(enc.windows(4).any(|w| w == b"0001"), "missing delim-pkt");
    }

    #[test]
    fn fetch_request_includes_wants() {
        let sha = "1111111111111111111111111111111111111111";
        let enc = encode_fetch_request(&[sha]);
        let s = String::from_utf8_lossy(&enc);
        assert!(s.contains("command=fetch"));
        assert!(s.contains(&format!("want {}", sha)));
        assert!(s.contains("done"));
        assert!(enc.ends_with(b"0000"));
    }

    #[test]
    fn parse_refs_advertisement_basic() {
        // Build a known-good response: HEAD + refs/heads/main, then flush.
        let head_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let main_sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let mut buf = Vec::new();
        buf.extend_from_slice(&encode_pkt_line(
            format!("{} HEAD symref-target:refs/heads/main\n", head_sha).as_bytes(),
        ));
        buf.extend_from_slice(&encode_pkt_line(
            format!("{} refs/heads/main\n", main_sha).as_bytes(),
        ));
        buf.extend_from_slice(FLUSH_PKT);
        let parsed = parse_refs_advertisement(&buf).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "HEAD");
        assert_eq!(parsed[0].symref_target.as_deref(), Some("refs/heads/main"));
        assert_eq!(parsed[1].name, "refs/heads/main");
        assert_eq!(parsed[1].sha, main_sha);
    }

    #[test]
    fn parse_refs_advertisement_skips_unknown_lines() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&encode_pkt_line(b"agent=git/2.43\n"));
        buf.extend_from_slice(FLUSH_PKT);
        let parsed = parse_refs_advertisement(&buf).unwrap();
        assert!(parsed.is_empty());
    }
}
