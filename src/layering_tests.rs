//! Architecture guards, asserted against the source so a stray import fails a
//! test instead of quietly inverting a boundary.
//!
//! The shape being enforced is a **vertical-slice** layout. Each domain slice
//! (`account`, `mail`) owns its full stack - `domain.rs` (pure centre), the
//! use-cases + `ports.rs` (functional core), and `infrastructure/` (framework). The
//! `platform` slice is the shared kernel and a **leaf**: it depends on no other
//! slice. `ui` is the driver: it may reach the slices and `platform`, but only
//! through their ports (never raw persistence). The composition root -
//! `main.rs` (which also owns the CLI definition), `daemon.rs`, and `wiring/` -
//! is intentionally NOT
//! scanned: it wires every slice together and so may name them all.
//!
//! This module lives at the crate root on purpose: it names forbidden patterns
//! as plain literals precisely because it is not inside any scanned slice, so
//! nothing here can match itself.

use std::path::{Path, PathBuf};

/// The scanned slices. `platform` is a leaf; `account`/`mail` are domain slices;
/// `ui` is the driver.
const SLICES: &[&str] = &["account", "mail", "platform", "ui"];

/// Framework/runtime substrings a slice's *core* (its `domain.rs`, `ports.rs`,
/// and use-case files) may not name. The `infrastructure/` subtree is exempt - that is
/// where the framework lives. `domain.rs` is even stricter (see [`scan`]).
const CORE_FORBIDDEN: &[&str] = &["ratatui", "sqlx::", "tokio::spawn("];

/// Cross-slice isolation: the `crate::<other>` roots a slice may never import.
/// A slice may always import `crate::platform` (the shared kernel), so platform
/// is absent from every non-platform list. `ui` may import the slices and
/// platform, so it only bans the composition root.
fn isolation_bans(slice: &str) -> &'static [&'static str] {
    match slice {
        "account" => &["crate::mail", "crate::ui", "crate::wiring", "crate::daemon", "crate::cli"],
        "mail" => &["crate::account", "crate::ui", "crate::wiring", "crate::daemon", "crate::cli"],
        // The kernel is a leaf: it may not reach into any slice or the root.
        "platform" => &[
            "crate::account",
            "crate::mail",
            "crate::ui",
            "crate::wiring",
            "crate::daemon",
            "crate::cli",
        ],
        // The driver reaches slices/platform through ports, but not the root.
        "ui" => &["crate::daemon", "crate::cli"],
        _ => &[],
    }
}

#[test]
fn slices_respect_boundaries() {
    let mut violations = Vec::new();
    for slice in SLICES {
        for offender in scan(slice) {
            violations.push(format!("{slice}: {offender}"));
        }
    }
    assert!(
        violations.is_empty(),
        "layering violations:\n{}",
        violations.join("\n")
    );
}

fn scan(slice: &str) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(slice);
    let mut out = Vec::new();
    for path in walk(&root) {
        // Integration tests under `ui/app/test` wire the real adapters over an
        // in-memory DB on purpose, so they are exempt.
        if slice == "ui" && path.components().any(|c| c.as_os_str() == "test") {
            continue;
        }
        let rel = path.strip_prefix(&root).unwrap_or(&path);
        let is_adapter = rel.components().any(|c| c.as_os_str() == "infrastructure");
        let is_domain = rel.as_os_str() == "domain.rs";
        // `platform` keeps its persistence primitives in `db/` and `secrets.rs`;
        // everywhere else in the kernel is sqlx-free like a slice core.
        let is_platform_persistence = slice == "platform"
            && (rel.starts_with("db") || rel.as_os_str() == "secrets.rs");

        // Assemble this file's ban list: cross-slice isolation always applies;
        // the intra-slice framework rules depend on which slice and subtree.
        let mut patterns: Vec<&str> = isolation_bans(slice).to_vec();
        match slice {
            // Domain slices: pure `domain.rs`, framework-free core, free adapters.
            "account" | "mail" => {
                if is_domain {
                    patterns.extend_from_slice(&[
                        "crate::", "sqlx::", "chrono::", "tokio::", "async_trait", "ratatui",
                    ]);
                } else if !is_adapter {
                    patterns.extend_from_slice(CORE_FORBIDDEN);
                }
            }
            // The kernel is UI-agnostic; sqlx is confined to its persistence corner.
            "platform" => {
                patterns.push("ratatui");
                if !is_platform_persistence {
                    patterns.push("sqlx::");
                }
            }
            // The driver may use ratatui and spawn tasks, but reaches adapters
            // through ports - never raw persistence.
            "ui" => patterns.push("sqlx::"),
            _ => {}
        }

        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (n, line) in src.lines().enumerate() {
            let trimmed = line.trim_start();
            // Doc comments and prose may name a forbidden thing; only real code
            // references count.
            if trimmed.starts_with("//") || trimmed.starts_with("#[") {
                continue;
            }
            for pat in &patterns {
                if trimmed.contains(pat) {
                    out.push(format!("{}:{}: `{pat}` -> {line}", path.display(), n + 1));
                }
            }
        }
    }
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}
