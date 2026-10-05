//! HTTP client that talks to the local daemon over its Unix socket.
//!
//! The daemon is already a Macula mesh client. The only job here is to
//! hand it a `{realm, repo_id, op, stdin}` payload and let the daemon
//! do the `macula:call` — we stay out of the QUIC business entirely.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hyper::{body::to_bytes, Body, Client, Method, Request, StatusCode};
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::url::MeshUrl;

/// Default path within `$HOME` when `HECATE_DAEMON_SOCKET` is not set.
const DEFAULT_SOCKET_REL: &str = ".hecate/hecate-daemon/sockets/api.sock";

/// Resolve the daemon socket path, honouring `HECATE_DAEMON_SOCKET`.
pub fn socket_path() -> Result<PathBuf> {
    if let Ok(explicit) = std::env::var("HECATE_DAEMON_SOCKET") {
        if !explicit.is_empty() {
            return Ok(PathBuf::from(explicit));
        }
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(DEFAULT_SOCKET_REL))
}

#[derive(Debug, Serialize)]
struct RpcRequest<'a> {
    realm: &'a str,
    repo_id: &'a str,
    op: &'a str,
    stdin_b64: String,
}

#[derive(Debug, Deserialize)]
struct RpcRawResponse {
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    stdout_b64: Option<String>,
    #[serde(default)]
    exit_status: Option<i64>,
    #[serde(default)]
    error: Option<String>,
    // Keep room for extension fields without blowing up deserialization.
    #[serde(flatten, default)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
pub struct CallResponse {
    pub ok: bool,
    pub stdout: Vec<u8>,
    /// Non-zero when the daemon's subprocess (git upload-pack /
    /// receive-pack) exited non-zero. Surfaced for debugging; the
    /// client path already consults `ok` + `error`.
    #[allow(dead_code)]
    pub exit_status: i64,
    pub error: Option<String>,
}

/// POST `/api/git/rpc` to the local daemon, get a decoded response back.
///
/// `stdin` is base64-encoded before send so the JSON transport is clean.
/// The daemon reciprocates with `stdout_b64`, which we decode here.
pub async fn call(mesh: &MeshUrl, op: &str, stdin: &[u8]) -> Result<CallResponse> {
    let sock = socket_path()?;
    if !sock.exists() {
        return Err(anyhow!(
            "hecate-daemon socket not found at {}. Is the daemon running?",
            sock.display()
        ));
    }

    let body = serde_json::to_vec(&RpcRequest {
        realm: &mesh.realm,
        repo_id: &mesh.repo_id,
        op,
        stdin_b64: B64.encode(stdin),
    })?;

    let uri: hyper::Uri = Uri::new(&sock, "/api/git/rpc").into();
    // hyperlocal synthesizes a hex-encoded Host header from the socket
    // path, which cowboy rejects (400) because of the `:0` port. Set an
    // explicit Host so cowboy's request parser accepts it.
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .body(Body::from(body))?;

    let client: Client<UnixConnector, Body> = Client::unix();
    let resp = client
        .request(req)
        .await
        .with_context(|| format!("POST /api/git/rpc via {}", sock.display()))?;

    let status = resp.status();
    let bytes = to_bytes(resp.into_body()).await?;

    if !(status.is_success() || status == StatusCode::BAD_REQUEST) {
        // Non-2xx + non-400: surface the body for debugging but don't
        // pretend it's a structured RPC reply.
        return Err(anyhow!(
            "daemon returned HTTP {}: {}",
            status.as_u16(),
            String::from_utf8_lossy(&bytes)
        ));
    }

    let raw: RpcRawResponse = serde_json::from_slice(&bytes)
        .with_context(|| format!("decoding daemon reply: {}", String::from_utf8_lossy(&bytes)))?;

    let stdout = match raw.stdout_b64 {
        Some(s) if !s.is_empty() => B64.decode(s).context("base64 decode of stdout_b64")?,
        _ => Vec::new(),
    };

    Ok(CallResponse {
        ok: raw.ok.unwrap_or(false),
        stdout,
        exit_status: raw.exit_status.unwrap_or(0),
        error: raw.error,
    })
}
