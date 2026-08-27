pub mod account;
pub mod mail;
pub mod oauth;
pub mod outcome;
pub mod ports;
pub mod services;

#[cfg(test)]
pub mod testing;

pub use mail::MailSource;
pub use services::Services;

#[cfg(test)]
mod layering_tests {
    /// Automated layering check: `application/` must stay a pure functional
    /// core - no shell/UI/event imports, no task spawning, no rendering.
    #[test]
    fn application_never_imports_shell_ui_event_or_spawns() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let root = std::path::Path::new(manifest).join("src/application");
        // Patterns are assembled at runtime so this test's own source (which
        // must mention them) never matches itself.
        let forbidden = [
            // `crate::tui::app::` - not `crate::tui::app`, which also prefixes
            // `crate::application`.
            ["crate::", "app::"].concat(),
            ["crate::", "view"].concat(),
            ["crate::", "event"].concat(),
            ["ratat", "ui"].concat(),
            ["tokio::sp", "awn("].concat(),
        ];
        let mut offenders = Vec::new();
        for entry in walk(&root) {
            let src = match std::fs::read_to_string(&entry) {
                Ok(s) => s,
                Err(_) => continue,
            };
            for line in src.lines() {
                let trimmed = line.trim_start();
                // Doc comments and prose mentioning the words are fine; only
                // real code references count.
                if trimmed.starts_with("//") || trimmed.starts_with("//!") {
                    continue;
                }
                for pat in &forbidden {
                    if trimmed.contains(pat) {
                        offenders.push(format!("{}: {line}", entry.display()));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "application layer violations:\n{}",
            offenders.join("\n")
        );
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
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
}
