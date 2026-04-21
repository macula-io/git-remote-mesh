//! git-remote-mesh — thin shim that teaches git to speak mesh://
//!
//! Git invokes us as `git-remote-mesh <remote-name> <url>` and speaks
//! the remote-helper protocol on stdio. We delegate all mesh work to
//! the local `hecate-daemon` over its Unix socket — there is no Rust
//! macula SDK, by design.

mod daemon;
mod helper;
mod stateless_rpc;
mod url;

use anyhow::{anyhow, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    // Git passes `<remote-name>` then `<url>`. The remote-name is only
    // useful for config lookups we don't currently do, but we accept it
    // so invocation is stable.
    let _remote_name = args.next();
    let raw_url = args
        .next()
        .ok_or_else(|| anyhow!("usage: git-remote-mesh <remote-name> <mesh://realm/repo_id>"))?;

    let mesh =
        url::MeshUrl::parse(&raw_url).map_err(|e| anyhow!("invalid mesh URL {raw_url:?}: {e}"))?;

    helper::run(mesh).await
}
