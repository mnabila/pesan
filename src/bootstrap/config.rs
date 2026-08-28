use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "pesan";
const CONFIG_FILE: &str = "config.yaml";

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", APP_NAME).context("unable to determine platform directories")
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().to_path_buf())
}

pub fn data_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().to_path_buf())
}

pub fn db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("pesan.db"))
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
    /// Load the app config from `~/.config/pesan/config.yaml`. On first run the
    /// directory and a documented default config are written. Missing optional
    /// sections fall back to defaults. Returns the loaded config.
    pub fn load() -> Result<Self> {
        let dir = config_dir()?;
        fs::create_dir_all(&dir).context("create config dir")?;

        let path = dir.join(CONFIG_FILE);
        if !path.exists() {
            fs::write(&path, DEFAULT_YAML)
                .with_context(|| format!("write default {}", path.display()))?;
            tracing::debug!("wrote default config to {}", path.display());
        }
        Self::from_file(&path)
    }

    /// Deserialize a config file through `config-rs` (YAML source). Kept separate
    /// so it can be exercised directly in tests.
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let settings = config::Config::builder()
            .add_source(config::File::from(path))
            .build()
            .with_context(|| format!("load {}", path.display()))?;
        let config: Self = settings
            .try_deserialize()
            .with_context(|| format!("parse {}", path.display()))?;
        tracing::debug!("config loaded from {}", path.display());
        Ok(config)
    }
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
    pub poll_interval_secs: u64,
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

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: "gruvbox".into(),
            ascii: false,
            layout: [2, 8],
            account_label: default_account_label(),
            border_type: default_border_type(),
            statusbar_position: default_statusbar_position(),
            poll_interval_secs: 120,
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
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            folders: vec!["INBOX".into()],
            show_sender: true,
            sound: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compose {
    pub editor: Option<String>,
    pub edit_headers: bool,
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
    /// parses through config-rs (`Config::from_file`), covering untagged enums
    /// (keybinding), fixed arrays (layout), and the custom themes map.
    #[test]
    fn default_yaml_loads_through_config_rs() {
        let dir = std::env::temp_dir().join(format!("pesan-cfg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        fs::write(&path, DEFAULT_YAML).unwrap();
        let parsed = Config::from_file(&path).unwrap();
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
}
