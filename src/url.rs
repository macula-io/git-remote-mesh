//! Parser for `mesh://<realm>/<repo_id>` URLs.
//!
//! The URL scheme is the contract between git and this helper:
//! git invokes `git-remote-mesh <remote-name> <url>`, and we have
//! to split `<url>` into the realm and the repo_id.
//!
//! We deliberately keep parsing strict — a malformed URL should
//! fail up-front, not silently produce a broken procedure MRI that
//! never resolves on the mesh.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshUrl {
    pub realm: String,
    pub repo_id: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UrlError {
    #[error("invalid scheme: {0} (expected `mesh`)")]
    InvalidScheme(String),
    #[error("missing realm (URL: {0})")]
    MissingRealm(String),
    #[error("missing repo_id (URL: {0})")]
    MissingRepoId(String),
    #[error("invalid repo_id: contains forbidden characters (URL: {0})")]
    InvalidRepoId(String),
    #[error("URL has extra path segments beyond realm/repo_id: {0}")]
    ExtraSegments(String),
    #[error("unparseable URL: {0}")]
    #[allow(dead_code)]
    Unparseable(String),
}

impl MeshUrl {
    /// Parse a `mesh://<realm>/<repo_id>` URL.
    ///
    /// We don't use the full `url::Url` parser because `mesh://` is a
    /// custom scheme and `url` is conservative about authority
    /// interpretation. Instead we do the minimal split ourselves.
    pub fn parse(raw: &str) -> Result<Self, UrlError> {
        let rest = match raw.strip_prefix("mesh://") {
            Some(r) => r,
            None => {
                // Try a softer error to help users who typed `mesh:`
                // without the authority slashes.
                let scheme = raw.split(':').next().unwrap_or("").to_string();
                return Err(UrlError::InvalidScheme(scheme));
            }
        };

        if rest.is_empty() {
            return Err(UrlError::MissingRealm(raw.to_string()));
        }

        // We accept exactly two path components: realm, repo_id.
        // Anything after is rejected rather than silently dropped, so
        // we don't encourage users to add `/refs/...` extensions that
        // we don't actually honour.
        let mut parts = rest.splitn(3, '/');
        let realm = parts.next().unwrap_or("");
        let repo_id = parts.next().unwrap_or("");
        let extra = parts.next();

        if realm.is_empty() {
            return Err(UrlError::MissingRealm(raw.to_string()));
        }
        if repo_id.is_empty() {
            return Err(UrlError::MissingRepoId(raw.to_string()));
        }
        if !is_valid_repo_id(repo_id) {
            return Err(UrlError::InvalidRepoId(raw.to_string()));
        }
        if let Some(tail) = extra {
            if !tail.is_empty() {
                return Err(UrlError::ExtraSegments(raw.to_string()));
            }
        }

        Ok(MeshUrl {
            realm: realm.to_string(),
            repo_id: repo_id.to_string(),
        })
    }

    /// The mesh procedure URI, exactly as `serve_git_over_mesh`
    /// advertises it: `<realm>.git.<repo_id>.rpc`.
    ///
    /// Not used directly by the helper (the daemon builds the URI on
    /// its side), but part of the public API for tests + potential
    /// debug output.
    #[allow(dead_code)]
    pub fn procedure_uri(&self) -> String {
        format!("{}.git.{}.rpc", self.realm, self.repo_id)
    }
}

fn is_valid_repo_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            // Printable ASCII, no slashes, no whitespace, no control chars.
            // UUIDv7 and BLAKE3-hex both fit this set.
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_url() {
        let u = MeshUrl::parse("mesh://io.macula/01HZY00000000000000000ABCD").unwrap();
        assert_eq!(u.realm, "io.macula");
        assert_eq!(u.repo_id, "01HZY00000000000000000ABCD");
        assert_eq!(
            u.procedure_uri(),
            "io.macula.git.01HZY00000000000000000ABCD.rpc"
        );
    }

    #[test]
    fn realm_with_dots_is_fine() {
        let u = MeshUrl::parse("mesh://alice.macula/config-repo").unwrap();
        assert_eq!(u.realm, "alice.macula");
        assert_eq!(u.repo_id, "config-repo");
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(matches!(
            MeshUrl::parse("https://example.com/foo"),
            Err(UrlError::InvalidScheme(_))
        ));
    }

    #[test]
    fn rejects_missing_repo_id() {
        assert!(matches!(
            MeshUrl::parse("mesh://io.macula/"),
            Err(UrlError::MissingRepoId(_))
        ));
        assert!(matches!(
            MeshUrl::parse("mesh://io.macula"),
            Err(UrlError::MissingRepoId(_))
        ));
    }

    #[test]
    fn rejects_missing_realm() {
        assert!(matches!(
            MeshUrl::parse("mesh:///foo"),
            Err(UrlError::MissingRealm(_))
        ));
        assert!(matches!(
            MeshUrl::parse("mesh://"),
            Err(UrlError::MissingRealm(_))
        ));
    }

    #[test]
    fn rejects_extra_segments() {
        assert!(matches!(
            MeshUrl::parse("mesh://io.macula/abc/refs/heads"),
            Err(UrlError::ExtraSegments(_))
        ));
    }

    #[test]
    fn rejects_invalid_repo_id() {
        assert!(matches!(
            MeshUrl::parse("mesh://io.macula/abc def"),
            Err(UrlError::InvalidRepoId(_))
        ));
        assert!(matches!(
            MeshUrl::parse("mesh://io.macula/abc\0def"),
            Err(UrlError::InvalidRepoId(_))
        ));
    }
}
