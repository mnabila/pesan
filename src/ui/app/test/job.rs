use super::*;
use crate::ui::app::event;
use crate::ui::view;

//
// Background job tracker
//

#[tokio::test]
async fn backtick_toggles_job_tracker() {
    let mut app = test_app().await;
    assert!(!app.jobs_open);
    // Backtick opens it (Global keymap binding).
    send_key!(app, Key::ch('`'));
    assert!(app.jobs_open, "backtick should open the job tracker");
    // Backtick again closes it (handled by the open-window key handler).
    send_key!(app, Key::ch('`'));
    assert!(!app.jobs_open, "backtick should close the job tracker");
    // Esc also closes it.
    send_key!(app, Key::ch('`'));
    assert!(app.jobs_open);
    send_key!(app, Key::esc());
    assert!(!app.jobs_open, "Esc should close the job tracker");
}

#[tokio::test]
async fn job_registry_tracks_lifecycle_and_progress() {
    let mut app = test_app().await;
    assert!(app.jobs.is_empty());

    // A registered job shows as running until it finishes.
    let id = app.jobs.begin(JobKind::FolderSync, "personal".to_string());
    assert_eq!(app.jobs.running_count(), 1);
    assert!(app.jobs.has_running());

    // Progress is reflected on the job.
    app.on_job_progress(event::JobProgress {
        id,
        done: 2,
        total: 5,
    });
    let job = app.jobs.iter().find(|j| j.id == id).unwrap();
    assert_eq!(job.progress, Some((2, 5)));
    assert_eq!(job.state, JobState::Running);

    // Completion flips it to Done and clears the running count.
    app.on_job_done(event::JobDone { id, error: None });
    let job = app.jobs.iter().find(|j| j.id == id).unwrap();
    assert_eq!(job.state, JobState::Done);
    assert_eq!(app.jobs.running_count(), 0);
    assert!(!app.jobs.has_running());
}

#[tokio::test]
async fn failed_job_records_error_and_clear_keeps_running() {
    let mut app = test_app().await;
    let done_id = app
        .jobs
        .begin(JobKind::FolderCounts, "personal".to_string());
    let fail_id = app.jobs.begin(JobKind::Connect, "personal".to_string());
    let run_id = app.jobs.begin(JobKind::FolderSync, "personal".to_string());

    app.on_job_done(event::JobDone {
        id: done_id,
        error: None,
    });
    app.on_job_done(event::JobDone {
        id: fail_id,
        error: Some("auth failed".to_string()),
    });

    let failed = app.jobs.iter().find(|j| j.id == fail_id).unwrap();
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error.as_deref(), Some("auth failed"));

    // Clearing finished jobs (the tracker's `c` key) drops Done + Failed but
    // keeps the still-running one.
    app.jobs_open = true;
    send_key!(app, Key::ch('c'));
    assert_eq!(app.jobs.running_count(), 1);
    assert!(app.jobs.iter().any(|j| j.id == run_id));
    assert!(app.jobs.iter().all(|j| j.state == JobState::Running));
}

#[tokio::test]
async fn job_tracker_renders_headed_table_with_progress_and_elapsed() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    let id = app.jobs.begin(JobKind::FolderSync, "personal".to_string());
    app.on_job_progress(event::JobProgress {
        id,
        done: 3,
        total: 8,
    });
    send_key!(app, Key::ch('`'));
    assert!(app.jobs_open);

    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let text: String = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();

    // Column headers are present.
    for header in ["JOB", "ACCOUNT", "PROGRESS", "ELAPSED"] {
        assert!(text.contains(header), "tracker header {header:?} shown");
    }
    // The running job's name, account, and progress count all render.
    assert!(text.contains("Sync folders"), "job name shown");
    assert!(text.contains("personal"), "account shown");
    assert!(text.contains("3/8"), "progress count shown");
    // The window title reports the running count and the key hint is present.
    assert!(text.contains("1 running"), "title shows running count");
    assert!(text.contains("clear finished"), "footer hint shown");
}

#[tokio::test]
async fn tick_prunes_finished_jobs_after_delay() {
    use std::time::Duration;
    let mut app = test_app().await;
    let id = app.jobs.begin(JobKind::FolderSync, "personal".to_string());
    app.on_job_done(event::JobDone { id, error: None });
    // A fresh finished job survives the 10s prune window in `tick`.
    app.tick();
    assert_eq!(app.jobs.iter().count(), 1);
    // But a zero-window prune drops it immediately (guards the retain logic).
    app.jobs.prune(Duration::from_secs(0));
    assert!(app.jobs.is_empty());
}
