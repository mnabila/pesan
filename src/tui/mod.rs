pub mod app;
pub mod keymap;
pub mod markdown;
pub mod theme;
pub mod views;
pub mod widgets;

#[cfg(test)]
mod layering_tests {
    /// Automated layering check: the TUI must depend only on `application` +
    /// `domain` (+ shared/bootstrap config), never on concrete
    /// `infrastructure` adapters. Test modules are exempt: integration tests
    /// wire real adapters over an in-memory DB by design.
    #[test]
    fn tui_never_imports_infrastructure() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let root = std::path::Path::new(manifest).join("src/tui");
        let mut offenders = Vec::new();
        for entry in walk(&root) {
            // Skip test code and this test's own source.
            if entry.to_string_lossy().contains("tests/") || entry == root.join("mod.rs") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&entry) else {
                continue;
            };
            for line in src.lines() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.starts_with("#[") {
                    continue;
                }
                if trimmed.contains("crate::infrastructure") {
                    offenders.push(format!("{}: {line}", entry.display()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "tui layer violations:\n{}",
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
