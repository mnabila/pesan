use std::time::Duration;

use anyhow::anyhow;
use crossterm::event::Event as CrosstermEvent;
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use crate::mail::{Envelope, Message};

/// Re-exported so app code can keep naming it from one place; defined in the
/// domain layer (see `domain::NewMail`).
pub use crate::mail::NewMail;

/// Onward messages to the app's single-threaded event loop. See the module
/// docs for the construction rule (app-only) and event groups.
#[derive(Debug)]
pub enum Event {
    Input(CrosstermEvent),
    Tick,
    Error(anyhow::Error),
    /// New messages observed by a background IMAP IDLE watcher.
    NewMail(NewMail),
    /// A background live-connect attempt finished (non-blocking connect).
    /// Boxed because `LiveData` is much larger than the other variants.
    Connected(Box<Connected>),
    /// A background folder refresh finished: fresh envelopes for `folder`,
    /// fetched off the UI loop so switching folders never blocks.
    FolderRefreshed(FolderRefreshed),
    /// A background message-body fetch finished, delivered off the UI loop so
    /// opening a not-yet-cached message never blocks.
    MessageFetched(Box<MessageFetched>),
    /// A "load older messages" page finished (pagination), delivered off the UI
    /// loop so paging back through a large mailbox never blocks.
    OlderLoaded(OlderLoaded),
    /// Folder (MESSAGES, UNSEEN) counts fetched in the background after a fast
    /// connect, so the per-folder STATUS chain never delays the initial display.
    FoldersCounted(FoldersCounted),
    /// Progress of the background all-folders envelope sync, so the status bar
    /// can show a determinate bar (`done`/`total` folders).
    SyncProgress(SyncProgress),
    /// A background mutation (e.g. optimistic bulk delete/archive) failed on the
    /// server; carries a human-readable message for a warning toast.
    BackgroundError(String),
    /// A live IMAP worker's session wedged (a command timed out) and shut down.
    /// Carries the account label so the app can auto-reconnect that account.
    ConnectionLost(String),
    /// Incremental progress of a tracked background job (job tracker window).
    JobProgress(JobProgress),
    /// A tracked background job finished (job tracker window); `error` is set if
    /// it failed. Sent by the job's RAII guard so a job never gets stuck running.
    JobDone(JobDone),
}

/// Incremental progress for a tracked background job (e.g. folders synced).
#[derive(Debug)]
pub struct JobProgress {
    pub id: u64,
    pub done: usize,
    pub total: usize,
}

/// Completion of a tracked background job; `error` carries the failure message
/// when the job failed, else `None`.
#[derive(Debug)]
pub struct JobDone {
    pub id: u64,
    pub error: Option<String>,
}

/// Per-folder `(name, total, unread)` counts from a background STATUS sweep.
#[derive(Debug)]
pub struct FoldersCounted {
    pub account: String,
    pub counts: Vec<(String, usize, usize)>,
}

/// Progress of the background all-folders envelope sync for one account.
#[derive(Debug, Clone)]
pub struct SyncProgress {
    pub account: String,
    pub done: usize,
    pub total: usize,
}

/// Result of a background folder refresh (live `list_messages`).
#[derive(Debug)]
pub struct FolderRefreshed {
    pub account: String,
    pub folder: String,
    pub envelopes: Vec<Envelope>,
}

/// Result of a "load older messages" pagination fetch: an older batch of
/// envelopes for `folder`, plus the server-side total so the app can tell
/// whether more remain. An empty batch with `total > 0` means the oldest
/// message has been reached; `total == 0` signals the fetch failed.
#[derive(Debug)]
pub struct OlderLoaded {
    pub account: String,
    pub folder: String,
    pub envelopes: Vec<Envelope>,
    pub total: u32,
}

/// Result of a background message-body fetch.
pub struct MessageFetched {
    pub account: String,
    pub folder: String,
    pub uid: u64,
    pub message: Message,
}

impl std::fmt::Debug for MessageFetched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageFetched")
            .field("account", &self.account)
            .field("folder", &self.folder)
            .field("uid", &self.uid)
            .finish()
    }
}

/// Outcome of a background live-connect for one account: the ready-to-use IMAP
/// source plus the initial mailbox load on success, or a typed error (`Auth`
/// means the stored credential was rejected and the app can re-authorize in
/// the background). Carried back to the app over the same event channel so the
/// UI never blocks on the (slow) connect *or* the initial folder/message sync.
pub struct Connected {
    pub account: String,
    pub result: Result<LiveData, ConnectError>,
}

pub use crate::wiring::connect::ConnectError;
/// The live source and the mail already fetched for it on the background task,
/// so [`crate::ui::app::App::on_connected`] can swap it in with no further network.
pub use crate::wiring::connect::LiveData;

impl std::fmt::Debug for Connected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connected")
            .field("account", &self.account)
            .field("ok", &self.result.is_ok())
            .finish()
    }
}

/// Commands the app acts on. The keymap resolves key sequences into these;
/// input widgets (compose fields, search box) handle their own keys inline and
/// only surface the app-level actions (send, close...) here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,

    // Navigation / selection
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    MoveFirst,
    MoveLast,
    FocusNext,
    FocusPrev,
    /// Directional focus movement within a form's field grid (compose headers,
    /// account edit form). Distinct from list `MoveUp`/`MoveDown` so forms can be
    /// rebound independently.
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,
    /// Start editing / activate the focused form field (text field -> type,
    /// compose body -> $EDITOR, provider -> chooser, toggle/button -> act).
    EditField,
    ExpandFolder,
    CollapseFolder,
    FilterAccounts,
    /// Account manager list actions (focus == Accounts).
    OpenAccount,
    AddAccount,
    DeleteAccount,
    SetDefaultAccount,

    // Reader scrolling
    ScrollUp,
    ScrollDown,
    ScrollHalfUp,
    ScrollHalfDown,
    ScrollStart,
    ScrollEnd,
    /// Toggle full RFC822 headers in the reader.
    ToggleHeaders,

    // Mail read path
    OpenMessage,
    Back,
    SelectFolder,
    ToggleSidebar,
    /// Re-fetch the currently selected mailbox from the live server.
    Refresh,

    // Mailbox actions
    ToggleUnread,
    ToggleFlag,
    /// Tag/untag the selected message in the list (Space) for bulk actions.
    ToggleMark,
    Delete,
    Archive,

    // Compose
    Compose,
    Reply,
    Forward,
    Send,
    SaveDraft,
    DiscardDraft,
    ExternalEditor,
    OpenAttachment,
    /// Render the message to /tmp/pesan/*.html and open it in the browser.
    OpenInBrowser,

    // Settings
    Settings,
    CloseSettings,
    SaveSettings,

    // Overlays
    Help,
    /// Toggle the background job tracker window.
    Jobs,
    Search,
    CommitSearch,
    CloseOverlay,
    SearchNext,
    SearchPrev,

    // Confirm dialog
    ConfirmYes,
    ConfirmNo,
}

const TICK_MS: u64 = 100;

/// Reader-thread control. `Pause` drops the `EventStream` (releasing stdin) so
/// an external program - the compose `$EDITOR` - can own the terminal without
/// this task stealing its keystrokes; the ack confirms the stream is gone.
/// `Resume` recreates the stream and starts reading again.
enum Control {
    Pause(oneshot::Sender<()>),
    Resume,
}

/// Spawns a background task that multiplexes the crossterm `EventStream` and a
/// render tick onto a single receiver channel. `tokio::select!` lives in the
/// spawned task; the app just awaits `next()`.
pub struct Events {
    rx: mpsc::UnboundedReceiver<Event>,
    tx: mpsc::UnboundedSender<Event>,
    ctrl_tx: mpsc::UnboundedSender<Control>,
}

impl Events {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<Control>();
        let forward = tx.clone();
        tokio::spawn(async move {
            let tx = forward;
            // Each pass owns a fresh `EventStream` (and its stdin reader thread).
            // A `Pause` drops it before we block waiting for `Resume`, so the
            // external editor has sole ownership of the terminal in between.
            'outer: loop {
                let mut stream = crossterm::event::EventStream::new();
                let mut tick = tokio::time::interval(Duration::from_millis(TICK_MS));
                loop {
                    tokio::select! {
                        maybe = stream.next() => match maybe {
                            Some(Ok(ev)) => {
                                if tx.send(Event::Input(ev)).is_err() { break 'outer; }
                            }
                            Some(Err(e)) => {
                                let _ = tx.send(Event::Error(anyhow::Error::new(e)));
                                break 'outer;
                            }
                            None => {
                                let _ = tx.send(Event::Error(anyhow!("event stream closed")));
                                break 'outer;
                            }
                        },
                        _ = tick.tick() => {
                            if tx.send(Event::Tick).is_err() { break 'outer; }
                        },
                        ctrl = ctrl_rx.recv() => match ctrl {
                            Some(Control::Pause(ack)) => {
                                // Release stdin, confirm, then wait for Resume.
                                drop(stream);
                                let _ = ack.send(());
                                loop {
                                    match ctrl_rx.recv().await {
                                        Some(Control::Resume) => continue 'outer,
                                        // A redundant pause while paused: just ack.
                                        Some(Control::Pause(a)) => { let _ = a.send(()); }
                                        None => break 'outer,
                                    }
                                }
                            }
                            // Not paused; a stray Resume is a no-op.
                            Some(Control::Resume) => {}
                            None => break 'outer,
                        },
                    }
                }
            }
        });
        Self { rx, tx, ctrl_tx }
    }

    /// A cloneable sender so background tasks (e.g. the IMAP IDLE watcher) can
    /// push events into the same loop the UI awaits.
    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    /// Stop reading terminal input and wait until the reader confirms stdin is
    /// released. Call this before handing the terminal to an external process.
    pub async fn pause_input(&self) {
        let (ack, ack_rx) = oneshot::channel();
        if self.ctrl_tx.send(Control::Pause(ack)).is_ok() {
            let _ = ack_rx.await;
        }
    }

    /// Resume reading terminal input after an external process returns.
    pub fn resume_input(&self) {
        let _ = self.ctrl_tx.send(Control::Resume);
    }
}
