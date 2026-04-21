# git-remote-mesh

A tiny `git` remote helper that teaches git to clone and fetch over the
Hecate/Macula mesh.

```bash
git clone mesh://io.macula/01HZY00000000000000000ABCD my-repo
```

## What it is

A single static binary that speaks the [git remote helper
protocol](https://git-scm.com/docs/gitremote-helpers) on stdio. When git
sees a `mesh://` URL, it searches `PATH` for `git-remote-mesh` and hands
off I/O to it.

This helper does **not** speak QUIC, DHT, or Macula RPC itself. It is
deliberately thin: it forwards git's stateless-rpc pkt-line payloads to
the **local** `hecate-daemon` over a Unix socket, and the daemon — which
is already a mesh client — performs the actual `macula:call` to the
target node.

```
┌────────────────┐   pkt-line   ┌─────────────────┐   HTTP/Unix   ┌───────────────┐   QUIC    ┌──────────────┐
│ git clone ...  │ ───────────▶ │ git-remote-mesh │ ────────────▶ │ hecate-daemon │ ─────────▶│ Macula relay │
└────────────────┘              └─────────────────┘               └───────────────┘           └──────────────┘
```

## Prerequisites

* A running `hecate-daemon` on the local machine. Every hecate-ish tool
  assumes this — `git-remote-mesh` is no exception.
* Rust 1.70+ to build from source.
* `git` (for `index-pack`).

## Install

From source:

```bash
cargo install --path .
# or:
cargo build --release
cp target/release/git-remote-mesh ~/.local/bin/
```

Once the binary is on `PATH`, git picks it up automatically:

```bash
git clone mesh://<realm>/<repo_id>
git fetch  # inside an existing mesh:// clone
```

## URL scheme

```
mesh://<realm>/<repo_id>
```

| Part      | Example                          | Notes                                      |
|-----------|----------------------------------|--------------------------------------------|
| `realm`   | `io.macula`                      | Realm identifier. No slashes.              |
| `repo_id` | `01HZY00000000000000000ABCD`     | UUIDv7 issued by `guide_repo_lifecycle`.   |

Copy the URL from the hecate-web `/git` catalog UI (landing in Phase 4).

### Human-name resolution — not yet

`mesh://did:realm:alice/config` style URLs are planned but not in v1 —
they need a DID-to-repo_id resolver endpoint on the daemon. Use raw
`repo_id` for now.

## Current limits

* **Clone + fetch only.** `git push` prints an explicit "push
  unsupported (Phase 3.1)" error per refspec — we refuse silently
  dropping data. Push lands next.
* **Whole-pack transfer.** Phase 2 of git-over-mesh does not stream
  pack bytes — the daemon buffers the full pack in memory before
  replying. Fine for small repos (config, personas); not a good
  choice for monorepos until Macula ships `call_stream`.
* **No human-name URLs yet.** See above.
* **Daemon must be running.** No fallback to a vanilla network git
  client; this helper is explicitly a mesh shim.

## Environment

| Variable                   | Purpose                                                     | Default                                       |
|----------------------------|-------------------------------------------------------------|-----------------------------------------------|
| `HECATE_DAEMON_SOCKET`     | Override the daemon Unix-socket path.                       | `$HOME/.hecate/hecate-daemon/sockets/api.sock` |

## Design doc

This helper is Phase 3 of
`hecate-social/hecate-station:plans/PLAN_GIT_OVER_MESH.md`. That plan
covers the full stack — aggregate, projection, mesh RPC server, this
helper, the Svelte browsing UX, and the macula-realm gitops
migration.

## Troubleshooting

**"hecate-daemon socket not found"** — the daemon isn't running, or it's
running with a non-default `HECATE_DAEMON_SOCKET`. Check with
`systemctl --user status hecate-daemon` (podman Quadlet deploy) or
whatever supervisor you use.

**"fetch failed: repo_not_on_disk"** — the target node has the repo
initiated in its event store but hasn't materialised a bare git dir on
disk yet. This is a daemon-side bug; see
`serve_git_over_mesh/initialize_repo_on_disk`.

**"no packfile section in fetch response"** — the server-side
`git upload-pack` run produced no pack. Happens if the repo is empty
(no commits) or the requested SHA isn't reachable.

## License

Apache-2.0. See [LICENSE](LICENSE).
