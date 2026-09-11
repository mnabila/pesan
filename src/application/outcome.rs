use crate::application::account::connect_params::ConnectParams;
use crate::domain::Envelope;

/// What opening a message should do: show `immediate` now (the cached copy or
/// the placeholder), then schedule `effect` (the SWR refetch, skipped when a
/// complete copy is already cached). Produced by `open::open_message`.
pub struct OpenOutcome {
    pub immediate: Option<crate::domain::Message>,
    pub effect: Option<Effect>,
}

/// Background work a use-case asks the shell to schedule.
#[derive(Debug)]
pub enum Effect {
    /// Re-list a folder from the live server, write it through to the cache,
    /// and deliver it as a folder-refreshed event. Schedule with
    /// `spawn_tracked` to wrap the round trip in a job-tracker entry that
    /// surfaces failures as a warning toast.
    RefreshFolder { folder: String },
    /// Fetch one message body, store it in the cache, and deliver it as a
    /// message-fetched event.
    FetchMessage { folder: String, env: Envelope },
    /// Warm the cache with the bodies of recently-arrived messages (low
    /// priority), so opening one is an instant cache hit. Each uid already
    /// fully cached is skipped; the rest are fetched with `BODY.PEEK` (no
    /// `\Seen`) on the background queue. Delivers no event - the bodies just
    /// land in SQLite.
    PrefetchBodies { folder: String, uids: Vec<u64> },
    /// Server-side deletes (or moves to `dest`, i.e. Archive), run low-priority
    /// after an optimistic local removal. Tracked as a job.
    DeleteOnServer {
        folder: String,
        uids: Vec<u64>,
        dest: Option<String>,
    },
    /// Fetch the next older page of a folder (pagination), write it through,
    /// and deliver it as an older-loaded event.
    LoadOlder { folder: String, offset: u32 },
    /// Sweep (MESSAGES, UNSEEN) counts for the given folder names. Tracked.
    FolderCounts { names: Vec<String> },
    /// Warm the cache with each listed folder's envelopes. Tracked; reports
    /// sync progress while running.
    SyncAllFolders { folders: Vec<String> },
    /// Run the full live connect + initial sync (the `connect_account`
    /// use-case) on a background task and deliver it as a connected event.
    /// Tracked (`Connect` for foreground, `Warmup` for background accounts).
    Connect {
        params: ConnectParams,
        want_folder: String,
        sync_all_folders: bool,
        /// Wedged-session notifier, relayed into a connection-lost event by an
        /// adapter task the shell keeps hand-spawned.
        on_lost: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    },
}
