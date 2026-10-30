//! Write-scope compilation and the conservative overlap check used by the
//! scheduler to keep concurrently running coder tasks from colliding.

use globset::{Glob, GlobSet, GlobSetBuilder};
use maestro_types::{MaestroError, Result};

/// Validate one scope pattern: relative, no `..` components, non-empty.
pub fn validate_scope(scope: &str) -> Result<()> {
    if scope.trim().is_empty() {
        return Err(MaestroError::Config("empty write scope".into()));
    }
    if scope.starts_with('/') {
        return Err(MaestroError::Config(format!(
            "write scope '{scope}' must be relative"
        )));
    }
    if scope.split('/').any(|c| c == "..") {
        return Err(MaestroError::Config(format!(
            "write scope '{scope}' must not contain '..'"
        )));
    }
    Ok(())
}

/// Compile scope patterns into a [`GlobSet`], rejecting absolute paths and
/// `..` components. An empty slice compiles to a set that matches nothing.
pub fn compile_scopes(scopes: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for scope in scopes {
        validate_scope(scope)?;
        let glob = Glob::new(scope)
            .map_err(|e| MaestroError::Config(format!("invalid write scope '{scope}': {e}")))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| MaestroError::Config(format!("cannot compile write scopes: {e}")))
}

/// True when `path` (repo-relative) matches at least one compiled scope.
pub fn path_in_scopes(scopes: &GlobSet, path: &str) -> bool {
    scopes.is_match(path)
}

/// Conservative static-prefix overlap check between two scope lists.
///
/// The static prefix of a pattern is everything before the first glob
/// metacharacter (`*?[{`). Two patterns overlap when either prefix is empty
/// or one prefix starts with the other. May report false positives, never
/// false negatives for prefix-shaped patterns.
pub fn scopes_overlap(a: &[String], b: &[String]) -> bool {
    for pa in a {
        let prefix_a = static_prefix(pa);
        for pb in b {
            let prefix_b = static_prefix(pb);
            if prefix_a.is_empty() || prefix_b.is_empty() {
                return true;
            }
            if prefix_a.starts_with(prefix_b) || prefix_b.starts_with(prefix_a) {
                return true;
            }
        }
    }
    false
}

/// Everything before the first glob metacharacter (`*?[{`).
pub fn static_prefix(pattern: &str) -> &str {
    match pattern.find(['*', '?', '[', '{']) {
        Some(i) => &pattern[..i],
        None => pattern,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn rejects_absolute_and_parent() {
        assert!(compile_scopes(&s(&["/etc/**"])).is_err());
        assert!(compile_scopes(&s(&["src/../secrets/**"])).is_err());
        assert!(compile_scopes(&s(&[".."])).is_err());
        assert!(compile_scopes(&s(&[""])).is_err());
        assert!(compile_scopes(&s(&["src/**", "docs/*.md"])).is_ok());
    }

    #[test]
    fn matching_works() {
        let set = compile_scopes(&s(&["src/**", "Cargo.toml"])).unwrap();
        assert!(path_in_scopes(&set, "src/main.rs"));
        assert!(path_in_scopes(&set, "src/deep/nested/mod.rs"));
        assert!(path_in_scopes(&set, "Cargo.toml"));
        assert!(!path_in_scopes(&set, "docs/readme.md"));
    }

    #[test]
    fn empty_scope_set_matches_nothing() {
        let set = compile_scopes(&[]).unwrap();
        assert!(!path_in_scopes(&set, "src/main.rs"));
    }

    #[test]
    fn overlap_is_prefix_based() {
        assert!(scopes_overlap(&s(&["src/**"]), &s(&["src/api/**"])));
        assert!(scopes_overlap(&s(&["src/api/**"]), &s(&["src/**"])));
        assert!(!scopes_overlap(&s(&["src/**"]), &s(&["docs/**"])));
        // Empty prefix (leading glob) overlaps everything.
        assert!(scopes_overlap(&s(&["**/*.rs"]), &s(&["docs/**"])));
        // Identical literals overlap.
        assert!(scopes_overlap(&s(&["Cargo.toml"]), &s(&["Cargo.toml"])));
        assert!(!scopes_overlap(&s(&["Cargo.toml"]), &s(&["Cargo.lock"])));
    }

    #[test]
    fn static_prefix_extraction() {
        assert_eq!(static_prefix("src/**"), "src/");
        assert_eq!(static_prefix("src/a.rs"), "src/a.rs");
        assert_eq!(static_prefix("**/*.rs"), "");
        assert_eq!(static_prefix("a{b,c}/d"), "a");
        assert_eq!(static_prefix("a?b"), "a");
    }
}
