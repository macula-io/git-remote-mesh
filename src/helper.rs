//! Git remote-helper protocol loop.
//!
//! See `gitremote-helpers(7)`. We consume line-oriented commands on
//! stdin and stream responses on stdout. For this MVP we implement:
//!
//!   * `capabilities`  — advertise what we support
//!   * `list`          — emit refs from the remote
//!   * `fetch <sha> <refname>` (may repeat; terminated by blank line)
//!   * `push …`        — currently unsupported; returns a clear error
//!
//! The pack-import phase shells out to `git index-pack --stdin` inside
//! `GIT_DIR`. Git sets that env var when invoking the helper; we trust
//! it rather than guessing based on cwd.

use anyhow::{anyhow, bail, Context, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::daemon;
use crate::stateless_rpc;
use crate::url::MeshUrl;

/// Run the protocol loop against the given remote.
pub async fn run(mesh: MeshUrl) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim_end_matches('\r').to_string();
        if line.is_empty() {
            // A blank line on its own signals "end of command batch" to
            // the helper. We just continue — each command arm flushes
            // its own terminator.
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");

        match cmd {
            "capabilities" => {
                // `fetch` enables git clone/fetch.
                // We explicitly omit `push` until Phase 3.1.
                stdout.write_all(b"fetch\n\n").await?;
                stdout.flush().await?;
            }
            "list" | "list for-push" => {
                handle_list(&mesh, &mut stdout).await?;
            }
            "fetch" => {
                // Collect all consecutive `fetch <sha> <ref>` lines, then
                // run a single pack request.
                let mut wants: Vec<String> = Vec::new();
                let (first_sha, _first_ref) = parse_fetch_args(rest)
                    .ok_or_else(|| anyhow!("malformed fetch command: {line:?}"))?;
                wants.push(first_sha);
                while let Some(next) = lines.next_line().await? {
                    let next = next.trim_end_matches('\r').to_string();
                    if next.is_empty() {
                        break;
                    }
                    let mut np = next.splitn(2, ' ');
                    let nc = np.next().unwrap_or("");
                    let nr = np.next().unwrap_or("");
                    if nc != "fetch" {
                        bail!("expected fetch line in batch, got: {next:?}");
                    }
                    let (sha, _) = parse_fetch_args(nr)
                        .ok_or_else(|| anyhow!("malformed fetch line: {next:?}"))?;
                    wants.push(sha);
                }
                handle_fetch_batch(&mesh, &wants, &mut stdout).await?;
            }
            "push" => {
                // Signal failure per-refspec so git reports cleanly.
                let refspec = rest.trim();
                let target = refspec.split(':').nth(1).unwrap_or(refspec);
                let msg = format!(
                    "error {} push unsupported (Phase 3.1 — see PLAN_GIT_OVER_MESH.md)\n\n",
                    target
                );
                stdout.write_all(msg.as_bytes()).await?;
                stdout.flush().await?;
            }
            "" => {
                // Empty after trim; ignore.
            }
            other => {
                // Unknown command — the helper protocol says to respond
                // with a blank line to indicate "not implemented".
                eprintln!("git-remote-mesh: unknown command {other:?}");
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
    }
    Ok(())
}

fn parse_fetch_args(rest: &str) -> Option<(String, String)> {
    let mut it = rest.splitn(2, ' ');
    let sha = it.next()?.to_string();
    let refname = it.next().unwrap_or("").to_string();
    if sha.is_empty() {
        None
    } else {
        Some((sha, refname))
    }
}

async fn handle_list<W: tokio::io::AsyncWrite + Unpin>(mesh: &MeshUrl, out: &mut W) -> Result<()> {
    let req = stateless_rpc::encode_ls_refs_request();
    let resp = daemon::call(mesh, "fetch", &req)
        .await
        .context("daemon RPC for ls-refs")?;
    if !resp.ok {
        bail!(
            "ls-refs failed: {}",
            resp.error.unwrap_or_else(|| "unknown".to_string())
        );
    }
    let refs = stateless_rpc::parse_refs_advertisement(&resp.stdout)
        .context("parsing ls-refs response")?;

    // Emit the refs in the format git expects from a helper's `list`.
    // HEAD gets special treatment: if it's symbolic, emit `@<target> HEAD`.
    for r in &refs {
        if r.name == "HEAD" {
            if let Some(target) = &r.symref_target {
                let line = format!("@{} HEAD\n", target);
                out.write_all(line.as_bytes()).await?;
                continue;
            }
        }
        let line = format!("{} {}\n", r.sha, r.name);
        out.write_all(line.as_bytes()).await?;
    }
    // Blank line terminates the list.
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

async fn handle_fetch_batch<W: tokio::io::AsyncWrite + Unpin>(
    mesh: &MeshUrl,
    wants: &[String],
    out: &mut W,
) -> Result<()> {
    let want_refs: Vec<&str> = wants.iter().map(String::as_str).collect();
    let req = stateless_rpc::encode_fetch_request(&want_refs);
    let resp = daemon::call(mesh, "fetch", &req)
        .await
        .context("daemon RPC for fetch")?;
    if !resp.ok {
        bail!(
            "fetch failed: {}",
            resp.error.unwrap_or_else(|| "unknown".to_string())
        );
    }

    // The response is a pkt-line stream containing an `acknowledgments`
    // or `packfile` section. The PACK bytes start after a `packfile\n`
    // pkt-line (or, when no negotiation happens, directly in sideband-1
    // framing). Rather than reimplementing the full v2 response parser
    // here, we pipe the complete response through `git index-pack
    // --stdin`, which understands both the surrounding pkt-line frames
    // and the PACK content. That matches what
    // `git fetch-pack --stateless-rpc` does internally.
    //
    // git index-pack needs a .git dir; we rely on GIT_DIR being set by
    // the parent `git clone` / `git fetch` process.
    let git_dir = std::env::var("GIT_DIR").context("GIT_DIR not set by git invocation")?;

    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(&git_dir)
        .arg("index-pack")
        .arg("--stdin")
        .arg("-v")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawning git index-pack")?;

    {
        let child_stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("index-pack stdin not available"))?;
        // Extract the pack stream from the v2 fetch response. The response
        // is pkt-line framed; we need the contents of the packfile section.
        let pack = extract_packfile(&resp.stdout)?;
        child_stdin.write_all(&pack).await?;
    }

    let status = child.wait().await?;
    if !status.success() {
        bail!("git index-pack exited with {}", status);
    }

    // Per remote-helper protocol, a single blank line after a fetch batch
    // signals "done, all refs delivered".
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

/// Extract the raw packfile bytes from a v2 `fetch` response.
///
/// v2 fetch responses come as pkt-line framed sections; we scan for the
/// `packfile\n` marker, then emit the concatenation of subsequent
/// sideband-1 channels (where byte 1 is `\x01` for pack data).
fn extract_packfile(bytes: &[u8]) -> Result<Vec<u8>> {
    // Minimal pkt-line walker.
    let mut pos = 0usize;
    let mut in_packfile = false;
    let mut out = Vec::new();
    while pos < bytes.len() {
        if bytes.len() < pos + 4 {
            bail!("truncated pkt-line length at offset {}", pos);
        }
        let len_str =
            std::str::from_utf8(&bytes[pos..pos + 4]).context("non-ascii pkt-line length")?;
        let len = u16::from_str_radix(len_str, 16)
            .with_context(|| format!("bad pkt-line length {len_str:?}"))?;
        pos += 4;
        if len == 0 {
            // flush
            if in_packfile {
                break;
            }
            continue;
        }
        if len == 1 {
            // delim
            continue;
        }
        let body_len = (len as usize).saturating_sub(4);
        if bytes.len() < pos + body_len {
            bail!("truncated pkt-line body at offset {}", pos);
        }
        let body = &bytes[pos..pos + body_len];
        pos += body_len;

        if !in_packfile {
            // Look for the section header `packfile\n`.
            let trimmed = body.strip_suffix(b"\n").unwrap_or(body);
            if trimmed == b"packfile" {
                in_packfile = true;
            }
            continue;
        }

        // In packfile section: first byte is the sideband channel.
        if body.is_empty() {
            continue;
        }
        match body[0] {
            1 => out.extend_from_slice(&body[1..]),
            2 => {
                // Progress — forward to stderr so the user sees it.
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &body[1..]);
            }
            3 => {
                let msg = String::from_utf8_lossy(&body[1..]).to_string();
                bail!("server sideband-3 error: {}", msg);
            }
            _ => {
                // Some servers use no sideband. Emit raw.
                out.extend_from_slice(body);
            }
        }
    }
    if out.is_empty() && !in_packfile {
        bail!("no packfile section in fetch response");
    }
    Ok(out)
}
