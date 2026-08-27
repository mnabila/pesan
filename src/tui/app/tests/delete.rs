use super::*;

#[tokio::test]
async fn delete_confirms_then_removes_row() {
    let mut app = test_app().await;
    let before = app.display_envelopes.len();
    send_key!(app, Key::ch('d')); // delete
    assert!(app.confirm.is_some());
    send_key!(app, Key::ch('y'));
    assert!(app.confirm.is_none());
    assert_eq!(app.display_envelopes.len(), before - 1);
}

#[tokio::test]
async fn delete_from_reader_removes_and_returns_to_list() {
    let mut app = test_app().await;
    let before = app.display_envelopes.len();
    // Open the selected message full-screen, then delete it with `#`.
    send_key!(app, Key::ch('o'));
    assert_eq!(app.view, View::Reader);
    send_key!(app, Key::ch('d'));
    assert!(app.confirm.is_some());
    send_key!(app, Key::ch('y'));
    assert!(app.confirm.is_none());
    assert_eq!(app.display_envelopes.len(), before - 1);
    // With the message gone there is nothing to read, so we drop back to the list.
    assert_eq!(app.view, View::Main);
    assert!(app.open_message.is_none());
}

#[tokio::test]
async fn archive_moves_message_to_archive_folder() {
    let mut app = test_app().await;
    let uid = app.selected_env().unwrap().uid;
    let before = app.display_envelopes.len();
    app.action(Action::Archive).await;
    assert_eq!(app.display_envelopes.len(), before - 1);
    // move_to relocated the row into the Archive mailbox in the cache.
    let archived = app.source.list_messages("Archive").await.unwrap();
    assert!(archived.iter().any(|e| e.uid == uid));
}

#[tokio::test]
async fn a_key_archives_from_list() {
    let mut app = test_app().await;
    let before = app.display_envelopes.len();
    send_key!(app, Key::ch('a')); // archive the selected message
    assert_eq!(app.display_envelopes.len(), before - 1);
}

#[tokio::test]
async fn space_toggles_mark() {
    let mut app = test_app().await;
    send_key!(app, Key::ch(' ')); // mark msg 0, cursor advances to 1
    assert_eq!(app.marked.len(), 1);
    send_key!(app, Key::ch('k')); // back to msg 0
    send_key!(app, Key::ch(' ')); // unmark msg 0
    assert!(app.marked.is_empty());
}

#[tokio::test]
async fn space_marks_then_d_deletes_all_marked() {
    let mut app = test_app().await;
    let before = app.display_envelopes.len();
    send_key!(app, Key::ch(' ')); // mark msg 0 (advances)
    send_key!(app, Key::ch(' ')); // mark msg 1 (advances)
    assert_eq!(app.marked.len(), 2);
    send_key!(app, Key::ch('d')); // delete -> confirm
    assert!(app.confirm.is_some());
    send_key!(app, Key::ch('y'));
    assert!(app.confirm.is_none());
    assert_eq!(app.display_envelopes.len(), before - 2);
    assert!(app.marked.is_empty(), "marks cleared after bulk delete");
}
