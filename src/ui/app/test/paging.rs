use super::*;
use crate::ui::app::event;

#[tokio::test]
async fn load_older_merges_dedups_and_marks_exhausted() {
    let mut app = test_app().await;
    assert_eq!(app.selected_folder_name(), "INBOX");
    assert_eq!(app.envelopes.len(), 5, "fixture inbox");
    // Highlight uid 3 so we can prove the cursor stays put across a merge.
    app.selected_message = 2;
    assert_eq!(app.selected_env().map(|e| e.uid), Some(3));

    // An older page: three new uids plus uid 5 (already loaded) to prove dedup.
    // total = 8 means the merged list holds everything the server has.
    app.on_older_loaded(event::OlderLoaded {
        account: "personal".to_string(),
        folder: "INBOX".to_string(),
        envelopes: vec![older_env(5), older_env(6), older_env(7), older_env(8)],
        total: 8,
    })
    .await;

    // 5 original + 3 new (uid 5 deduped), still newest-first, cursor preserved.
    assert_eq!(app.envelopes.len(), 8);
    assert_eq!(app.display_envelopes.len(), 8);
    let uids: Vec<u64> = app.envelopes.iter().map(|e| e.uid).collect();
    assert_eq!(uids, vec![1, 2, 3, 4, 5, 6, 7, 8], "sorted newest-first");
    assert_eq!(app.selected_env().map(|e| e.uid), Some(3), "cursor kept");
    assert!(!app.loading_older);
    assert!(app.older_exhausted, "all server messages now loaded");
}

#[tokio::test]
async fn load_older_empty_page_marks_exhausted_but_error_does_not() {
    let mut app = test_app().await;
    app.loading_older = true;
    // Empty page with a real total: we've reached the oldest message.
    app.on_older_loaded(event::OlderLoaded {
        account: "personal".to_string(),
        folder: "INBOX".to_string(),
        envelopes: Vec::new(),
        total: 5,
    })
    .await;
    assert!(!app.loading_older);
    assert!(app.older_exhausted);
    assert_eq!(app.envelopes.len(), 5, "list unchanged");

    // A failed fetch (total 0, no rows) clears the flag but leaves paging on.
    app.older_exhausted = false;
    app.loading_older = true;
    app.on_older_loaded(event::OlderLoaded {
        account: "personal".to_string(),
        folder: "INBOX".to_string(),
        envelopes: Vec::new(),
        total: 0,
    })
    .await;
    assert!(!app.loading_older);
    assert!(!app.older_exhausted, "error keeps paging enabled for retry");
}

#[tokio::test]
async fn load_older_ignores_stale_folder_page() {
    let mut app = test_app().await;
    app.loading_older = true;
    // A page that arrives for a folder the user has since left is dropped.
    app.on_older_loaded(event::OlderLoaded {
        account: "personal".to_string(),
        folder: "Archive".to_string(),
        envelopes: vec![older_env(6), older_env(7)],
        total: 20,
    })
    .await;
    assert_eq!(app.envelopes.len(), 5, "stale page ignored");
    assert!(!app.older_exhausted);
}

#[tokio::test]
async fn scroll_paging_is_noop_when_offline() {
    let mut app = test_app().await;
    // Scrolling down triggers the infinite-scroll prefetch. The fixture app is a
    // CacheSource (not live), so paging does nothing and never wedges the flag.
    for _ in 0..app.envelopes.len() {
        app.action(event::Action::MoveDown).await;
    }
    assert!(!app.loading_older);
    assert_eq!(app.envelopes.len(), 5);
}
