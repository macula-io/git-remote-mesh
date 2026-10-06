# Changelog

All notable changes to `git-remote-mesh` are documented here.

## Retired — 2026-10-06

The repository is archived. Its only backend, `hecate-daemon`, is retired and no mesh service serves git
repositories, so the helper cannot work. See the README.

## 0.1.0 — 2026-04-21

Initial release. Clone-only MVP for Phase 3 of PLAN_GIT_OVER_MESH.

### Added

- `mesh://<realm>/<repo_id>` URL parser with strict validation.
- git remote-helper protocol loop: `capabilities`, `list`, `fetch`.
- git protocol v2 pkt-line encoders for `ls-refs` and `fetch`.
- Ref-advertisement parser (handles `symref-target:` for HEAD).
- v2 fetch-response pack extractor (sideband-1 demuxing).
- Unix-socket HTTP client for the local `hecate-daemon`.
- `HECATE_DAEMON_SOCKET` override.

### Known limits

- `git push` returns "unsupported (Phase 3.1)". No silent no-op.
- Whole-pack transfers — no streaming (gated on `macula:call_stream`).
- Raw `repo_id` URLs only — no DID/human-name resolution yet.
