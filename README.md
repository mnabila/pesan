# pesan

A fast, keyboard-driven terminal email client. `pesan` reads over IMAP and sends over SMTP, authenticates with OAuth2 (Gmail / Outlook), caches everything locally so it opens instantly and works offline, and gets out of your way with a vim-style keymap.

- **Offline-first** - mail is cached in SQLite; the UI shows cached content immediately, then syncs live in the background.
- **Push new mail** - IMAP IDLE (with a polling fallback) plus desktop notifications.
- **Secure by default** - only refresh tokens are stored, and they live in your OS keyring, never on disk in plaintext.
- **Configurable** - YAML config for providers, layout, themes, notifications, and key bindings.

---

## Install

### Prerequisites

- **Rust** (2024 edition; a recent stable toolchain). Install via [rustup](https://rustup.rs/).
- **An OS keyring / secret service** for token storage (recommended, not required):
  - Linux: a Secret Service provider (e.g. GNOME Keyring or KWallet) must be running.
  - macOS: Keychain (built in).
  - Windows: Credential Manager (built in).
  - If no keyring is available (e.g. headless Linux with no D-Bus/Secret Service), pesan
    automatically falls back to a `secrets` table in the app database (`pesan.db`). The
    token is stored in plaintext there - a security downgrade from the encrypted OS keychain.
- **A terminal** with 256-color / true-color support recommended.

### Build from source

```bash
git clone <this-repo> pesan
cd pesan
cargo build --release
# binary at ./target/release/pesan
```

To install it on your `PATH`:

```bash
cargo install --path .
```

Or just run it during development:

```bash
cargo run
```

> The first build is slow: dependencies are compiled with optimizations even in dev profile so the app itself stays responsive.

### Commands

Running `pesan` with no command opens the interactive TUI. A few read-only subcommands are available for scripting and quick checks:

```bash
pesan                    # launch the interactive TUI (default)
pesan version            # print the version and exit
pesan account list       # list configured accounts (name, email, provider, default, authorized)
pesan account info       # details for the default account
pesan account info NAME  # details for a specific account
pesan help               # usage
```

---

## First run

```bash
pesan
```

On first launch, pesan writes a documented default config to `~/.config/pesan/config.yaml` (path varies by OS - see [File locations](#file-locations)) and opens with no accounts. Before you can read mail you need to (1) give pesan OAuth app credentials for your provider, and (2) add and authorize an account.

### 1. Set up OAuth app credentials

pesan uses OAuth2 - you never enter your email password. You provide the OAuth **application** credentials once per provider in `config.yaml`; each user account then authorizes through the browser.

#### How the OAuth2 flow works

pesan uses the **authorization-code flow with PKCE**, with a **manual (copy/paste) redirect** - pesan never runs a local web server. When you authorize an account it:

1. Opens your browser to the provider's consent page.
2. After you approve, the provider redirects your browser to `http://localhost` with the authorization code in the URL. **Nothing is listening there**, so the page fails to load - that's expected.
3. You copy that URL from the browser's address bar and paste it into pesan (Enter). pesan reads the `code`/`state` query params, validates them, exchanges the code for tokens, and stores **only the refresh token** in your OS keyring.

Register your provider app as an **installed / desktop / public client** with `http://localhost` in its allowed redirect URIs (accepted by Google/Microsoft loopback registrations). The redirect URI is not configurable - pesan always uses `http://localhost`. Press **Esc** on the paste prompt to cancel.

#### The `oauth` config fields

Each provider has an `oauth` block under `providers.<name>.oauth` in `config.yaml`:

```yaml
providers:
  gmail:
    oauth:
      auth_url:      "https://accounts.google.com/o/oauth2/v2/auth"  # provider authorize endpoint
      token_url:     "https://oauth2.googleapis.com/token"           # provider token endpoint
      scopes:        ["https://mail.google.com/"]                    # IMAP/SMTP access scopes
      client_id:     "xxxx.apps.googleusercontent.com"              # from your provider app
      client_secret: "yyyy"                                          # blank "" for public/PKCE apps
```

- `auth_url` / `token_url` / `scopes` are pre-filled correctly for the built-in `gmail` and `outlook` providers - you normally only fill in `client_id` and `client_secret`.
- There is **no** `redirect_uri` field: pesan always advertises `http://localhost` (it binds no listener), and after consent you paste the redirect URL back in. Register `http://localhost` on your OAuth app.
- `client_secret` may be left blank (`""`) for public/desktop apps that use PKCE (e.g. Azure public clients). For a Google Desktop-app client the secret is required but is non-confidential for installed apps.
- pesan automatically requests offline access (Google gets `access_type=offline` + `prompt=consent`) so the provider returns a refresh token, plus the `openid`/`email`/`profile` scopes so it can read your address and display name to pre-fill the account (leave **Name** blank to use your real account name).

#### Gmail

Create OAuth credentials in the [Google Cloud Console](https://console.cloud.google.com/):

1. Create a project and enable the **Gmail API** (APIs & Services → Library → Gmail API → Enable).
2. Configure the **OAuth consent screen** (External is fine for personal use). While the app is in "Testing", add your Google address under **Test users**.
3. Create credentials → **OAuth client ID** → application type **Desktop app**.
4. Copy the generated **Client ID** and **Client secret** into `providers.gmail.oauth` in `config.yaml`.

```yaml
providers:
  gmail:
    imap: { host: imap.gmail.com, port: 993 }
    smtp: { host: smtp.gmail.com, port: 465 }
    oauth:
      auth_url: "https://accounts.google.com/o/oauth2/v2/auth"
      token_url: "https://oauth2.googleapis.com/token"
      scopes: ["https://mail.google.com/"]
      client_id: "YOUR_ID.apps.googleusercontent.com"
      client_secret: "YOUR_SECRET"
```

> The `https://mail.google.com/` scope is the full IMAP/SMTP access scope Gmail requires; narrower scopes will not work for IMAP.

#### Outlook / Microsoft 365

Register an app in the [Azure portal](https://portal.azure.com/) → **Microsoft Entra ID** → **App registrations** → **New registration**:

1. Under **Authentication**, add a platform → **Mobile and desktop applications**. This makes it a **public client** (PKCE, no secret). Enable the "allow public client flows" option if prompted.
2. Under **API permissions**, add delegated permissions for IMAP/SMTP and offline access - these match the scopes in the default config:
   - `https://outlook.office.com/IMAP.AccessAsUser.All`
   - `https://outlook.office.com/SMTP.Send`
   - `offline_access`
3. Copy the **Application (client) ID** into `providers.outlook.oauth.client_id`; leave `client_secret` blank.

```yaml
providers:
  outlook:
    imap: { host: outlook.office365.com, port: 993 }
    smtp: { host: smtp.office365.com, port: 587 }
    oauth:
      auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
      token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token"
      scopes:
        - "https://outlook.office.com/IMAP.AccessAsUser.All"
        - "https://outlook.office.com/SMTP.Send"
        - "offline_access"
      client_id: "00000000-0000-0000-0000-000000000000"
      client_secret: ""
```

> Some Microsoft 365 tenants disable IMAP/SMTP OAuth by default; if authorization succeeds but login fails, ask your tenant admin to enable OAuth (modern auth) for IMAP and SMTP.

#### Keeping secrets out of the config file

Any `oauth` string value may reference an environment variable with `${VAR}`, expanded when the config loads. This keeps the client secret out of the file:

```yaml
providers:
  gmail:
    oauth:
      client_secret: "${PESAN_GMAIL_SECRET}"   # read from the environment at load time
```

Then export it before launching, e.g. `export PESAN_GMAIL_SECRET=...` in your shell profile.

#### Adding another provider

To use a provider other than Gmail/Outlook (e.g. Fastmail), add a new key under `providers` with its `imap`, `smtp`, and `oauth` settings, then reference that key as the account's **Provider**.

### 2. Add an account

In the app, press **`S`** to open Settings (Account Manager), then:

- Add a new account and fill in **Name**, **Email**, and pick the **Provider** (matching a key under `providers` in your config).
- Optionally mark it as the **default** account.
- Trigger **Authorize** to open the browser consent flow, then **paste the redirect URL** the browser lands on back into pesan (Enter). Save with **`W`**.

Field navigation uses `Tab`/`j`/`k`; press `e` to start typing into the focused field and `Esc`/`Enter` to stop. Save with `W`, cancel with `Esc`.

---

## Configuration

Config lives at `~/.config/pesan/config.yaml` (see [File locations](#file-locations)). Unknown fields are rejected, so typos surface as load errors. The file is fully commented; see [`example.config.yaml`](example.config.yaml) for the annotated reference. Summary:

**Modular drop-ins.** Any `*.yaml`/`*.yml` file placed in `~/.config/pesan/config.d/` is merged over `config.yaml`, sorted by file name (later files win). Tables deep-merge and scalars/arrays are replaced, so a fragment can add or override individual keys without duplicating the rest - e.g. keep providers in `config.d/providers.yaml` and keybindings in `config.d/keymap.yaml`. The `config.d/` directory is created empty on first run.


### `providers`

OAuth app + server definitions, shared across accounts. Each account references a provider by key. Hosts and OAuth credentials are defined **only** here (never duplicated in the database).

```yaml
providers:
  gmail:
    imap: { host: imap.gmail.com, port: 993 }
    smtp: { host: smtp.gmail.com, port: 465 }
    oauth:
      auth_url: "https://accounts.google.com/o/oauth2/v2/auth"
      token_url: "https://oauth2.googleapis.com/token"
      scopes: ["https://mail.google.com/"]
      client_id: "xxxx.apps.googleusercontent.com"
      client_secret: "yyyy"          # or "${PESAN_GMAIL_SECRET}"
```

`gmail` and `outlook` are pre-populated with the correct hosts and scopes; you only need to fill in credentials. Add more providers (e.g. Fastmail) as new keys.

### `ui`

```yaml
ui:
  theme: gruvbox            # dark | light | mono | any custom theme name
  ascii: false              # true = ASCII glyph fallback (no unicode icons)
  layout: [2, 8]             # [sidebar, main] pane-width ratio; only the sidebar slot is used (messages open full-screen)
  account_label: name       # sidebar account label: name | email
  border_type: plain        # plain | rounded | thick | double
  poll_interval_secs: 120   # background refresh fallback when IMAP IDLE is unavailable
```

### `notifications`

```yaml
notifications:
  enabled: true             # desktop notification on new mail
  folders: [INBOX]          # which mailboxes to watch
  show_sender: true         # include From/subject in the notification body
  sound: false              # play a sound with the notification
  # sound_file: ~/.config/pesan/new-mail.oga  # specific sound file when sound is true; unset => default theme sound
```

`sound_file` is passed to the notification daemon via the freedesktop `sound-file` hint (`~` is expanded to your home directory), so playback needs a daemon that supports sound hints (dunst, GNOME, KDE). With `sound: true` and no `sound_file`, a default theme sound is used.

### `compose`

```yaml
compose:
  editor: null              # external editor cmd; null => $VISUAL, then $EDITOR, then "vi"
                            # e.g. "nvim", or with args: "code --wait"
  edit_headers: false       # true = editor buffer also includes To/Cc/Subject headers
```

### `themes`

Define your own color themes and select one via `ui.theme`. Every role is required; values are `#rrggbb` hex, a named color (e.g. `blue`), or an indexed `0-255`. The built-in names `dark` / `light` / `mono` are reserved.

```yaml
themes:
  gruvbox:
    bg: "#282828"
    fg: "#ebdbb2"
    dim: "#928374"
    accent: "#83a598"
    unread: "#fabd2f"
    success: "#b8bb26"
    warning: "#fe8019"
    error: "#fb4934"
    border: "#3c3836"
    border_focus: "#83a598"
```

### `keybinding`

Remap any action to your own keys, grouped by keymap section. A section only affects bindings active in that context: `global` works everywhere; the others apply while that pane/screen has focus. Sections: `global`, `folders` (alias `sidebar`), `list`, `reader`, `compose`, `settings`, `search`, `confirm`. Key syntax: a single key, a chord such as `gg`, or `ctrl+<key>` / `shift+<key>` / `alt+<key>`. Special names: `enter`, `esc`, `space`, `tab`, `up`, `down`, `left`, `right`, `home`, `end`, `pgup`, `pgdn`, `delete`, `backspace`. Each action takes a single sequence or a list (first match wins). An override replaces all built-in keys for the action within that section; other sections keep their built-ins. Unknown sections, actions, or unparseable keys are ignored.

```yaml
keybinding:
  global:               # work in every context
    quit: q
    help: "?"
    jobs: "`"
    settings: S
    focus_next: L
    focus_prev: H
    toggle_sidebar: z
    refresh: R
  folders:              # sidebar: folder tree / account switcher
    select_folder: o
    expand_folder: l
    collapse_folder: h
    filter_accounts: /
  list:                 # message list
    open_message: [enter, o]
    open_in_browser: O
    compose: c
    reply: r
    forward: f
    delete: d
    archive: a
    search: /
    back: esc
  reader:               # full-screen message view
    reply: r
    forward: f
    open_attachment: o
    open_in_browser: O
    delete: d
    archive: a
    back: esc
  compose:              # compose form (body edited in $EDITOR)
    send: ctrl+s
    save_draft: ctrl+d
    external_editor: ctrl+e
    discard_draft: esc
  settings:
    save_settings: W
    close_settings: esc
  search:
    commit_search: enter
    close_overlay: esc
  confirm:              # yes/no dialog
    confirm_yes: [y, enter]
    confirm_no: [n, esc]
```

---

## Key bindings

Press **`?`** anytime for a context-aware help overlay. Defaults:

**Global**

| Key | Action |
|-----|--------|
| `q` | quit |
| `?` | toggle help |
| `S` | settings / account manager |
| `H` / `L` | focus pane left / right |
| `z` | collapse sidebar |
| `R` | refresh the current mailbox |

**Sidebar** (folders / accounts)

| Key | Action |
|-----|--------|
| `j` / `k` | down / up |
| `gg` / `G` | first / last |
| `o` | open mailbox / switch account |
| `l` / `h` (or `→` / `←`) | expand / collapse |
| `/` | filter accounts |

**Message list**

| Key | Action |
|-----|--------|
| `j` / `k`, `gg` / `G` | navigate |
| `o` / `Enter` | open message (full-screen reader) |
| `O` | open message in browser (`/tmp/pesan/*.html`) |
| `Space` | mark / unmark (tag for bulk actions) |
| `u` | toggle unread |
| `s` | flag / star |
| `d` | delete marked, or the selected message (with confirm) |
| `a` | archive marked, or the selected message |
| `/`, `n` / `N` | search, next / prev match |
| `c` / `r` / `f` | compose / reply / forward |
| `Esc` | back to folders |

**Reader**

| Key | Action |
|-----|--------|
| `j` / `k` | scroll down / up |
| `gg` / `G` | top / bottom |
| `r` / `f` | reply / forward |
| `o` | open attachment |
| `O` | open message in browser (`/tmp/pesan/*.html`) |
| `h` | toggle full message headers |
| `d` | delete (with confirm) |
| `a` | archive |
| `Esc` | back to list |

**Compose** (headers edited inline; body opens in `$EDITOR`)

| Key | Action |
|-----|--------|
| `Tab` / `j` / `k` | move field |
| `e` | edit focused field / body |
| `Ctrl-e` | edit body in `$EDITOR` |
| `Ctrl-s` | send |
| `Ctrl-d` | save draft |
| `Esc` | discard |

**Settings** — `Tab` next field, `W` save, `Esc` back. **Confirm dialogs** — `y`/`Enter` confirm, `n`/`Esc` cancel.

---

## File locations

Paths follow platform conventions (via the `directories` crate). On Linux:

| What | Path |
|------|------|
| Config | `~/.config/pesan/config.yaml` |
| Local cache DB | `~/.local/share/pesan/pesan.db` |
| Logs | `~/.local/share/pesan/logs/pesan.log` |
| Tokens | OS keyring (service `pesan`), or the `secrets` table in `pesan.db` if no keyring |

macOS and Windows use their respective app-directory conventions.

---

## Troubleshooting

- **Logs:** set the log level with `PESAN_LOG` (or `RUST_LOG`), e.g. `PESAN_LOG=pesan=debug pesan`. Logs are written to the log file only (never stdout, which the TUI owns).
- **"provider returned no refresh token":** your OAuth consent needs offline access and a consent prompt - re-run authorization; for Google, ensure your address is an allowed test user.
- **Token no longer valid:** pesan re-authorizes automatically: when the stored credential is rejected it opens the browser consent flow and prompts you to paste the redirect URL back (once per session per account). Press **Esc** on the paste prompt to cancel (e.g. if you closed the browser tab). You can also re-authorize manually via Settings (`S`) → **Authorize**.
- **No desktop notifications on Linux:** ensure a notification daemon and a Secret Service provider are running.
- **Icons look wrong:** set `ui.ascii: true` for an ASCII glyph fallback.

---

## License

Licensed under the [MIT License](LICENSE).
