use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "pesan";
const CONFIG_FILE: &str = "config.yaml";
/// Drop-in directory: every `*.yaml`/`*.yml` fragment here is merged over
/// `config.yaml` (sorted by file name, later files win), `/etc/*.d`-style.
const CONFIG_D_DIR: &str = "config.d";

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", APP_NAME).context("unable to determine platform directories")
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().to_path_buf())
}

/// The `config.d/` drop-in directory under the config dir.
pub fn config_d_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join(CONFIG_D_DIR))
}

pub fn data_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().to_path_buf())
}

pub fn db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("pesan.db"))
}

/// Unix socket the `pesan daemon` listens on and the TUI client dials. Prefers
/// the XDG runtime dir (`$XDG_RUNTIME_DIR/pesan/daemon.sock`, tmpfs, cleared on
/// logout), falling back to the data dir on platforms without one.
pub fn socket_path() -> Result<PathBuf> {
    let dir = project_dirs()?
        .runtime_dir()
        .map(Path::to_path_buf)
        .unwrap_or(data_dir()?);
    Ok(dir.join("daemon.sock"))
}

pub fn log_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("logs"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub providers: HashMap<String, Provider>,
    #[serde(default)]
    pub ui: Ui,
    #[serde(default)]
    pub notifications: Notifications,
    #[serde(default)]
    pub daemon: Daemon,
    #[serde(default)]
    pub compose: Compose,
    /// Per-context key overrides, grouped by keymap section:
    /// section (`global`, `list`, ...) -> action name -> one or more sequences.
    #[serde(default)]
    pub keybinding: KeyBindings,
    /// User-defined color themes, keyed by name. Selected via `ui.theme`. The
    /// built-in names (dark/light/mono) are reserved and take precedence, so a
    /// custom theme sharing one of those names is ignored.
    #[serde(default)]
    pub themes: HashMap<String, ThemeSpec>,
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = HashMap::new();
        providers.insert("gmail".to_string(), Provider::gmail());
        providers.insert("outlook".to_string(), Provider::outlook());
        let mut themes = HashMap::new();
        themes.insert("gruvbox".to_string(), ThemeSpec::gruvbox());
        Self {
            providers,
            ui: Ui::default(),
            notifications: Notifications::default(),
            daemon: Daemon::default(),
            compose: Compose::default(),
            keybinding: HashMap::new(),
            themes,
        }
    }
}

/// A user-defined theme. Every color role is required; each value is a color
/// string parsed by ratatui (`#rrggbb` hex, a named color like `blue`, or an
/// indexed `0-255`). An unparseable value falls back to the built-in dark
/// theme's color for that role at load time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ThemeSpec {
    pub bg: String,
    pub fg: String,
    pub dim: String,
    pub accent: String,
    pub unread: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub border: String,
    pub border_focus: String,
}

impl ThemeSpec {
    /// The gruvbox (dark) palette, the shipped default theme.
    pub fn gruvbox() -> Self {
        Self {
            bg: "#282828".into(),
            fg: "#ebdbb2".into(),
            dim: "#928374".into(),
            accent: "#83a598".into(),
            unread: "#fabd2f".into(),
            success: "#b8bb26".into(),
            warning: "#fe8019".into(),
            error: "#fb4934".into(),
            border: "#3c3836".into(),
            border_focus: "#83a598".into(),
        }
    }
}

/// A key override value: either a single key sequence or a list of them.
/// Example keybinds:
///   quit: q
///   open_message: [enter, o]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum KeySpecs {
    One(String),
    Many(Vec<String>),
}

/// The `keybinding` config table: section name (`global`, `list`, `compose`,
/// ...) -> action name -> key sequence(s). Sections map to keymap contexts
/// (`tui::keymap::Ctx::from_section`).
pub type KeyBindings = HashMap<String, HashMap<String, KeySpecs>>;

impl KeySpecs {
    /// The user-typed key sequences, in order.
    pub fn sequences(&self) -> Vec<&str> {
        match self {
            KeySpecs::One(s) => vec![s.as_str()],
            KeySpecs::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

impl Config {
    /// Load the app config from `~/.config/pesan/config.yaml`, merging any
    /// `config.d/*.yaml` drop-in fragments on top (sorted by file name, later
    /// wins). On first run the config dir, a documented default `config.yaml`,
    /// and an empty `config.d/` directory are written. Missing optional sections
    /// fall back to defaults. Returns the loaded config.
    pub fn load() -> Result<Self> {
        let dir = config_dir()?;
        fs::create_dir_all(&dir).context("create config dir")?;

        let path = dir.join(CONFIG_FILE);
        if !path.exists() {
            fs::write(&path, DEFAULT_YAML)
                .with_context(|| format!("write default {}", path.display()))?;
            tracing::debug!("wrote default config to {}", path.display());
        }

        // Ensure the drop-in dir exists so users can discover it, then merge any
        // fragments it holds over the base file.
        let config_d = config_d_dir()?;
        fs::create_dir_all(&config_d).context("create config.d dir")?;
        let fragments = config_d_fragments(&config_d);
        Self::from_sources(&path, &fragments)
    }

    /// Deserialize `base` and merge each of `fragments` (in order) over it via
    /// `config-rs`, which deep-merges tables and replaces scalars/arrays, so a
    /// fragment can add or override individual nested keys. `#[serde(deny_unknown_fields)]`
    /// still guards the merged result, so a stray key in any file errors out.
    pub fn from_sources(base: &Path, fragments: &[PathBuf]) -> Result<Self> {
        let mut builder = config::Config::builder().add_source(config::File::from(base));
        for frag in fragments {
            builder = builder.add_source(config::File::from(frag.as_path()));
        }
        let settings = builder
            .build()
            .with_context(|| format!("load {}", base.display()))?;
        let config: Self = settings
            .try_deserialize()
            .with_context(|| format!("parse {}", base.display()))?;
        tracing::debug!(
            "config loaded from {} (+{} fragment(s))",
            base.display(),
            fragments.len()
        );
        Ok(config)
    }
}

/// Collect the `*.yaml`/`*.yml` fragments in a `config.d/` directory, sorted by
/// file name so ordering (e.g. `10-ui.yaml` before `20-work.yaml`) is
/// deterministic. Returns empty when the directory is absent or unreadable -
/// a missing drop-in dir is not an error.
fn config_d_fragments(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml")
                })
        })
        .collect();
    files.sort();
    files
}

// Note: there is intentionally no `save()`. `config.yaml` is user-owned (it
// carries comments, custom keybinds, and themes); a full serde round-trip would
// strip all of that. The file is only ever written once, on first run, in
// `load()`. Runtime-toggleable preferences (e.g. layout) stay in-memory for the
// session; permanent changes are made by editing the file directly.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub imap: Server,
    pub smtp: Server,
    /// OAuth2 app + endpoints. Omit the whole block for a password-login
    /// provider: the account then authenticates with IMAP/SMTP LOGIN using a
    /// password stored in the keyring, and the servers come from `imap`/`smtp`.
    #[serde(default)]
    pub oauth: Option<OAuth>,
}

impl Default for Provider {
    fn default() -> Self {
        Self::gmail()
    }
}

impl Provider {
    /// True when this provider authenticates via OAuth2 (has an `oauth` block).
    /// A provider without one is a password-login provider.
    pub fn is_oauth(&self) -> bool {
        self.oauth.is_some()
    }

    /// Gmail server + OAuth defaults (credentials left blank for the user).
    pub fn gmail() -> Self {
        Self {
            imap: Server {
                host: "imap.gmail.com".into(),
                port: 993,
            },
            smtp: Server {
                host: "smtp.gmail.com".into(),
                port: 465,
            },
            oauth: Some(OAuth {
                auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
                token_url: "https://oauth2.googleapis.com/token".into(),
                scopes: vec!["https://mail.google.com/".into()],
                client_id: String::new(),
                client_secret: String::new(),
            }),
        }
    }

    /// Outlook / Microsoft 365 server + OAuth defaults (credentials left blank).
    pub fn outlook() -> Self {
        Self {
            imap: Server {
                host: "outlook.office365.com".into(),
                port: 993,
            },
            smtp: Server {
                host: "smtp.office365.com".into(),
                port: 587,
            },
            oauth: Some(OAuth {
                auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
                token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
                scopes: vec![
                    "https://outlook.office.com/IMAP.AccessAsUser.All".into(),
                    "https://outlook.office.com/SMTP.Send".into(),
                    "offline_access".into(),
                ],
                client_id: String::new(),
                client_secret: String::new(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuth {
    pub auth_url: String,
    pub token_url: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ui {
    pub theme: String,
    pub ascii: bool,
    /// pane sizes: [sidebar, main]. Only the sidebar width is
    /// used now (messages open in a full-screen reader); the list/reader slots
    /// are kept for backward compatibility and ignored.
    pub layout: [u8; 2],
    /// What to show for each account in the sidebar: `name` or `email`.
    #[serde(default = "default_account_label")]
    pub account_label: String,
    /// Pane border weight: `plain` (thin), `rounded`, `thick` (heavy), or
    /// `double`. Terminal borders are always one cell thick; this changes the
    /// line-drawing style, not the cell width.
    #[serde(default = "default_border_type")]
    pub border_type: String,
    /// Where the single statusbar sits: `top` or `bottom`.
    #[serde(default = "default_statusbar_position")]
    pub statusbar_position: String,
}

fn default_account_label() -> String {
    "name".to_string()
}

fn default_border_type() -> String {
    "plain".to_string()
}

fn default_statusbar_position() -> String {
    "bottom".to_string()
}

fn default_prefetch_today_count() -> usize {
    30
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: "gruvbox".into(),
            ascii: false,
            layout: [2, 8],
            account_label: default_account_label(),
            border_type: default_border_type(),
            statusbar_position: default_statusbar_position(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notifications {
    pub enabled: bool,
    #[serde(default)]
    pub folders: Vec<String>,
    pub show_sender: bool,
    pub sound: bool,
    /// Optional path to a sound file played with the notification (via the
    /// freedesktop `sound-file` hint) when `sound` is true. `~` is expanded to
    /// the home directory. When unset, `sound: true` falls back to a default
    /// theme sound. Ignored entirely when `sound` is false.
    #[serde(default)]
    pub sound_file: Option<String>,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            folders: vec!["INBOX".into()],
            show_sender: true,
            sound: false,
            sound_file: None,
        }
    }
}

/// Fallback freedesktop sound-theme name used when `sound: true` but no
/// `sound_file` is configured.
const DEFAULT_SOUND_NAME: &str = "message-new-instant";

impl Notifications {
    /// Resolve the configured sound into a [`SoundHint`], or `None` when sound
    /// is disabled. A configured `sound_file` (with `~` expanded) becomes a
    /// file hint; otherwise a default theme-sound name is used.
    pub fn sound_hint(&self) -> Option<crate::application::ports::SoundHint> {
        use crate::application::ports::SoundHint;
        if !self.sound {
            return None;
        }
        Some(match &self.sound_file {
            Some(path) if !path.is_empty() => SoundHint::File(expand_tilde(path)),
            _ => SoundHint::Name(DEFAULT_SOUND_NAME.to_string()),
        })
    }
}

/// Expand a leading `~` / `~/` to the user's home directory. Any other path is
/// returned unchanged. Home is resolved via the same `directories` crate used
/// for the config path.
fn expand_tilde(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) else {
        return path.to_string();
    };
    // `~` alone, or `~/...` - strip the separator so join doesn't double it.
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    home.join(rest).to_string_lossy().into_owned()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compose {
    pub editor: Option<String>,
    pub edit_headers: bool,
}

fn default_true() -> bool {
    true
}

/// Settings for `pesan daemon` (the headless auto-fetch process). Every knob
/// falls back to an existing setting when left at its default, so the daemon
/// works with no `daemon:` block at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Daemon {
    /// Folders to watch and prefetch per account. Empty falls back to
    /// [`Notifications::folders`], then to `INBOX`.
    #[serde(default)]
    pub folders: Vec<String>,
    /// Poll cadence (seconds) for the daemon's watcher when IMAP IDLE is
    /// unavailable. Floored at 15s. This is the single poll knob - only the
    /// daemon watches.
    #[serde(default = "default_poll_secs")]
    pub poll_interval_secs: u64,
    /// Warm message bodies into the cache (today's mail on connect, and new
    /// arrivals). `false` syncs envelopes + notifies only, fetching no bodies.
    #[serde(default = "default_true")]
    pub prefetch_bodies: bool,
    /// Max of today's newest bodies prefetched per folder (on connect and on new
    /// arrivals). `0` disables; ignored when `prefetch_bodies` is false.
    #[serde(default = "default_prefetch_today_count")]
    pub prefetch_today_count: usize,
    /// Raise a desktop notification on new mail. Uses [`Notifications`] settings
    /// (`show_sender`, `sound`, `sound_file`) for the notification's content.
    #[serde(default = "default_true")]
    pub notify: bool,
    /// Restrict the daemon to these account names. Empty = every authorized account.
    #[serde(default)]
    pub accounts: Vec<String>,
    /// Periodically re-list each watched folder this often (seconds) to pick up
    /// server-side changes (flag updates, removals) beyond IDLE's new-mail signal,
    /// pushing the refreshed list to connected clients. `0` disables the sweep.
    #[serde(default = "default_resync_secs")]
    pub resync_secs: u64,
}

fn default_resync_secs() -> u64 {
    300
}

fn default_poll_secs() -> u64 {
    120
}

impl Default for Daemon {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            poll_interval_secs: default_poll_secs(),
            prefetch_bodies: true,
            prefetch_today_count: default_prefetch_today_count(),
            notify: true,
            accounts: Vec::new(),
            resync_secs: default_resync_secs(),
        }
    }
}

impl Daemon {
    /// Folders to watch: configured `daemon.folders`, else `notifications.folders`,
    /// else `INBOX`.
    pub fn resolved_folders(&self, notifications: &Notifications) -> Vec<String> {
        if !self.folders.is_empty() {
            self.folders.clone()
        } else if !notifications.folders.is_empty() {
            notifications.folders.clone()
        } else {
            vec!["INBOX".to_string()]
        }
    }

    /// The daemon watcher's poll cadence, floored at 15s.
    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.poll_interval_secs.max(15))
    }

    /// Effective body-prefetch cap: `0` (disabled) when `prefetch_bodies` is
    /// false, else `prefetch_today_count`.
    pub fn prefetch_cap(&self) -> usize {
        if self.prefetch_bodies {
            self.prefetch_today_count
        } else {
            0
        }
    }

    /// Whether an account (by name) is in scope: all when the list is empty.
    pub fn includes(&self, account_name: &str) -> bool {
        self.accounts.is_empty() || self.accounts.iter().any(|a| a == account_name)
    }

    /// Periodic folder re-sync cadence, or `None` when disabled (`resync_secs=0`).
    /// Floored at 30s to keep the sweep gentle on the server.
    pub fn resync_interval(&self) -> Option<std::time::Duration> {
        (self.resync_secs > 0).then(|| std::time::Duration::from_secs(self.resync_secs.max(30)))
    }
}

/// The documented default config written to `config.yaml` on first run,
/// embedded from the repo-root `example.config.yaml` at compile time.
const DEFAULT_YAML: &str = include_str!("../../example.config.yaml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips_through_yaml() {
        let config = Config::default();
        let yaml = serde_yaml_ng::to_string(&config).unwrap();
        let parsed: Config = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(parsed.ui.theme, "gruvbox");
        assert!(!parsed.ui.ascii);
        assert_eq!(parsed.ui.layout, [2, 8]);
        assert_eq!(parsed.ui.statusbar_position, "bottom");
        assert_eq!(parsed.notifications.folders, vec!["INBOX"]);
        assert!(parsed.providers.contains_key("gmail"));
        assert!(parsed.providers.contains_key("outlook"));
        assert!(parsed.themes.contains_key("gruvbox"));
    }

    #[test]
    fn default_provides_gmail_and_outlook() {
        let config = Config::default();
        let outlook = &config.providers["outlook"];
        assert_eq!(outlook.imap.host, "outlook.office365.com");
        assert_eq!(outlook.smtp.host, "smtp.office365.com");
        assert_eq!(outlook.smtp.port, 587);
        assert!(
            outlook
                .oauth
                .as_ref()
                .unwrap()
                .token_url
                .contains("login.microsoftonline.com")
        );
        // The default theme resolves to a defined custom theme, not a fallback.
        assert!(config.themes.contains_key(&config.ui.theme));
    }

    #[test]
    fn custom_theme_roundtrips_through_yaml() {
        let mut config = Config::default();
        config.themes.insert(
            "nord".to_string(),
            ThemeSpec {
                bg: "#2e3440".into(),
                fg: "#d8dee9".into(),
                dim: "#4c566a".into(),
                accent: "#88c0d0".into(),
                unread: "#ebcb8b".into(),
                success: "#a3be8c".into(),
                warning: "#d08770".into(),
                error: "#bf616a".into(),
                border: "#3b4252".into(),
                border_focus: "#88c0d0".into(),
            },
        );
        let yaml = serde_yaml_ng::to_string(&config).unwrap();
        let parsed: Config = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(parsed.themes["nord"].bg, "#2e3440");
        assert_eq!(parsed.themes["nord"].border_focus, "#88c0d0");
    }

    #[test]
    fn default_yaml_defines_gruvbox_theme() {
        let parsed: Config = serde_yaml_ng::from_str(DEFAULT_YAML).unwrap();
        let gruvbox = &parsed.themes["gruvbox"];
        assert_eq!(gruvbox.bg, "#282828");
        assert_eq!(gruvbox.accent, "#83a598");
    }

    #[test]
    fn default_yaml_parses() {
        let parsed: Config = serde_yaml_ng::from_str(DEFAULT_YAML).unwrap();
        let gmail = &parsed.providers["gmail"];
        assert_eq!(gmail.imap.host, "imap.gmail.com");
        assert_eq!(gmail.imap.port, 993);
        assert_eq!(gmail.smtp.port, 465);
        assert_eq!(
            gmail.oauth.as_ref().unwrap().scopes,
            vec!["https://mail.google.com/"]
        );
        assert_eq!(
            parsed.keybinding["list"]["compose"].sequences(),
            vec!["c".to_string()],
        );
        assert_eq!(
            parsed.keybinding["list"]["open_message"].sequences(),
            vec!["enter".to_string(), "o".to_string()],
        );
    }

    /// End-to-end check of the real load path: the documented default config
    /// parses through config-rs (`Config::from_sources`), covering untagged enums
    /// (keybinding), fixed arrays (layout), and the custom themes map.
    #[test]
    fn default_yaml_loads_through_config_rs() {
        let dir = std::env::temp_dir().join(format!("pesan-cfg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        fs::write(&path, DEFAULT_YAML).unwrap();
        let parsed = Config::from_sources(&path, &[]).unwrap();
        assert_eq!(parsed.ui.theme, "gruvbox");
        assert_eq!(parsed.ui.layout, [2, 8]);
        assert_eq!(parsed.ui.border_type, "plain");
        assert_eq!(parsed.ui.statusbar_position, "bottom");
        assert_eq!(parsed.providers["gmail"].imap.port, 993);
        assert_eq!(parsed.providers["outlook"].smtp.port, 587);
        assert_eq!(parsed.themes["gruvbox"].bg, "#282828");
        assert_eq!(
            parsed.keybinding["list"]["open_message"].sequences(),
            vec!["enter".to_string(), "o".to_string()],
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// A per-test scratch dir with the default `config.yaml` written as the base.
    /// Returns (dir, base path). Caller writes fragments into `dir/config.d`.
    fn scratch(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("pesan-cfgd-{}-{tag}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(dir.join(CONFIG_D_DIR)).unwrap();
        let base = dir.join(CONFIG_FILE);
        fs::write(&base, DEFAULT_YAML).unwrap();
        (dir, base)
    }

    #[test]
    fn fragment_overrides_base_scalar_and_keeps_siblings() {
        let (dir, base) = scratch("override");
        fs::write(
            dir.join(CONFIG_D_DIR).join("10-ui.yaml"),
            "ui:\n  theme: light\n",
        )
        .unwrap();
        let frags = config_d_fragments(&dir.join(CONFIG_D_DIR));
        let parsed = Config::from_sources(&base, &frags).unwrap();
        // Overridden leaf wins...
        assert_eq!(parsed.ui.theme, "light");
        // ...and untouched base `ui` fields survive (deep merge, not replace).
        assert_eq!(parsed.ui.layout, [2, 8]);
        assert_eq!(parsed.ui.statusbar_position, "bottom");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fragment_adds_to_providers_map() {
        let (dir, base) = scratch("providers");
        fs::write(
            dir.join(CONFIG_D_DIR).join("work.yaml"),
            "providers:\n  work:\n    imap:\n      host: imap.work.test\n      port: 993\n    smtp:\n      host: smtp.work.test\n      port: 465\n",
        )
        .unwrap();
        let frags = config_d_fragments(&dir.join(CONFIG_D_DIR));
        let parsed = Config::from_sources(&base, &frags).unwrap();
        // Base providers survive and the fragment's is added.
        assert!(parsed.providers.contains_key("gmail"));
        assert!(parsed.providers.contains_key("outlook"));
        assert_eq!(parsed.providers["work"].imap.host, "imap.work.test");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn later_fragment_wins_by_filename_order() {
        let (dir, base) = scratch("order");
        let d = dir.join(CONFIG_D_DIR);
        fs::write(d.join("20-late.yaml"), "ui:\n  theme: late\n").unwrap();
        fs::write(d.join("10-early.yaml"), "ui:\n  theme: early\n").unwrap();
        let frags = config_d_fragments(&d);
        // Discovery is sorted by name regardless of write order.
        assert!(frags[0].ends_with("10-early.yaml"));
        assert!(frags[1].ends_with("20-late.yaml"));
        let parsed = Config::from_sources(&base, &frags).unwrap();
        assert_eq!(parsed.ui.theme, "late");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_or_empty_config_d_loads_base_only() {
        let (dir, base) = scratch("empty");
        // Empty config.d (created by scratch, no fragments) -> base only.
        let frags = config_d_fragments(&dir.join(CONFIG_D_DIR));
        assert!(frags.is_empty());
        let parsed = Config::from_sources(&base, &frags).unwrap();
        assert_eq!(parsed.ui.theme, "gruvbox");
        // A nonexistent dir is also fine (not an error).
        assert!(config_d_fragments(&dir.join("nope.d")).is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sound_hint_resolves_by_config() {
        use crate::application::ports::SoundHint;
        // Disabled -> no hint.
        let n = Notifications {
            sound: false,
            sound_file: Some("/x.oga".into()),
            ..Notifications::default()
        };
        assert_eq!(n.sound_hint(), None);
        // Enabled, no file -> default theme name.
        let n = Notifications {
            sound: true,
            sound_file: None,
            ..Notifications::default()
        };
        assert_eq!(
            n.sound_hint(),
            Some(SoundHint::Name(DEFAULT_SOUND_NAME.into()))
        );
        // Enabled with an absolute file -> file hint, path unchanged.
        let n = Notifications {
            sound: true,
            sound_file: Some("/sounds/new.oga".into()),
            ..Notifications::default()
        };
        assert_eq!(
            n.sound_hint(),
            Some(SoundHint::File("/sounds/new.oga".into()))
        );
    }

    #[test]
    fn expand_tilde_expands_home_only() {
        assert_eq!(expand_tilde("/abs/path"), "/abs/path");
        assert_eq!(expand_tilde("rel/path"), "rel/path");
        if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
            assert_eq!(
                expand_tilde("~/x.oga"),
                home.join("x.oga").to_string_lossy().into_owned()
            );
        }
    }

    #[test]
    fn unknown_key_in_fragment_errors() {
        let (dir, base) = scratch("unknown");
        fs::write(
            dir.join(CONFIG_D_DIR).join("bad.yaml"),
            "not_a_real_section: 1\n",
        )
        .unwrap();
        let frags = config_d_fragments(&dir.join(CONFIG_D_DIR));
        // deny_unknown_fields still guards the merged value.
        assert!(Config::from_sources(&base, &frags).is_err());
        fs::remove_dir_all(&dir).ok();
    }
}
