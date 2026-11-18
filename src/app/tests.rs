//! UI tests: real widget interactions through `iced_test::Simulator`.

use super::format::*;
use super::*;
use chrono::TimeZone;
use iced::Theme;
use lbx_timer::provider::{HubstaffSnapshot, HubstaffStatus, ObserverConfig, ProviderError};
use lbx_timer::{Event, EventCause, EventKind, TaskState, TimerState, MAX_TIMERS};
use std::time::{Duration, Instant};

const SIZE: (f32, f32) = WINDOW_SIZE;
/// Viewport for driving clicks: tall enough that targets a user would
/// scroll to are on screen. Geometry checks use the real window `SIZE`.
const TALL: (f32, f32) = (WINDOW_SIZE.0, 2_000.0);

struct TempDb(std::path::PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        Self(std::env::temp_dir().join(format!("lbx-ui-{name}-{}.db", uuid::Uuid::new_v4())))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", ".lock", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

fn at(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .expect("valid timestamp")
}

fn deliver(app: &mut App, messages: Vec<Message>) {
    for message in messages {
        let _ = app.update(message);
    }
}

fn click_text(app: &mut App, label: &str) {
    let messages = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), TALL, app.view());
        ui.click(label)
            .unwrap_or_else(|error| panic!("could not click {label:?}: {error}"));
        ui.into_messages().collect::<Vec<_>>()
    };
    deliver(app, messages);
}

fn click_id(app: &mut App, id: &str) {
    let messages = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), TALL, app.view());
        ui.click(iced_test::selector::id(id.to_owned()))
            .unwrap_or_else(|error| panic!("could not click widget {id:?}: {error}"));
        ui.into_messages().collect::<Vec<_>>()
    };
    deliver(app, messages);
}

fn has_text(app: &App, label: &str) -> bool {
    iced_test::Simulator::with_size(Default::default(), SIZE, app.view())
        .find(label)
        .is_ok()
}

fn has_id_at(app: &App, id: &str, size: (f32, f32)) -> bool {
    iced_test::Simulator::with_size(Default::default(), size, app.view())
        .find(iced_test::selector::id(id.to_owned()))
        .is_ok()
}

fn has_id(app: &App, id: &str) -> bool {
    iced_test::Simulator::with_size(Default::default(), SIZE, app.view())
        .find(iced_test::selector::id(id.to_owned()))
        .is_ok()
}

fn assert_text(app: &App, label: &str) {
    assert!(
        has_text(app, label),
        "expected the rendered view to contain {label:?}"
    );
}

fn bounds_at(app: &App, id: &str, size: (f32, f32)) -> iced::Rectangle {
    let mut ui = iced_test::Simulator::with_size(Default::default(), size, app.view());
    ui.find(iced_test::selector::id(id.to_owned()))
        .unwrap_or_else(|error| panic!("expected widget {id:?}: {error}"))
        .bounds()
}

fn bounds_for(app: &App, id: &str) -> iced::Rectangle {
    bounds_at(app, id, SIZE)
}

fn snapshot(app: &App, size: (f32, f32), name: &str) {
    let mut ui = iced_test::Simulator::with_size(Default::default(), size, app.view());
    assert!(ui
        .snapshot(&Theme::Dark)
        .unwrap()
        .matches_image(format!("target/ui-snapshots/{name}.png"))
        .unwrap());
}

fn type_name(app: &mut App, value: &str) {
    let messages = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), TALL, app.view());
        ui.click(iced_test::selector::id("task-name"))
            .expect("task name input should be present");
        let _ = ui.typewrite(value);
        ui.into_messages().collect::<Vec<_>>()
    };
    deliver(app, messages);
}

/// Stored events of the timer shown in Details.
fn selected_events(app: &App) -> Vec<Event> {
    app.selected_id
        .as_deref()
        .map(|id| engine(app).events(id).unwrap())
        .unwrap_or_default()
}

fn engine(app: &App) -> &TimerEngine {
    app.engine.as_ref().expect("engine is open")
}

fn timer_ids(app: &App) -> Vec<String> {
    engine(app).timers().iter().map(|t| t.id.clone()).collect()
}

fn state_of(app: &App, id: &str) -> TimerState {
    engine(app).timer(id).expect("timer exists").state
}

fn hubstaff_app(name: &str) -> (TempDb, App) {
    let db = TempDb::new(name);
    let app = App::at_path_with_observer(
        db.0.clone(),
        ObserverConfig::Hubstaff(std::path::PathBuf::from("/fake/HubstaffCLI.bin.x86_64")),
    );
    (db, app)
}

fn report_hubstaff(
    app: &mut App,
    start_intent: Option<u64>,
    result: Result<HubstaffSnapshot, ProviderError>,
) {
    app.hubstaff_poll_id = app.hubstaff_poll_id.wrapping_add(1);
    let request_id = app.hubstaff_poll_id;
    app.hubstaff_poll_pending = true;
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent,
        result,
    });
}

fn start_hubstaff_timer(app: &mut App, id: &str) -> u64 {
    let _ = app.update(Message::Start(id.to_owned()));
    assert!(
        engine(app).running().is_none(),
        "the direct Start message must wait for fresh Hubstaff state"
    );
    let intent = app
        .pending_start
        .as_ref()
        .map(|(_, intent)| *intent)
        .expect("start intent waits for a fresh query");
    let request_id = app.hubstaff_poll_id;
    assert!(app.hubstaff_poll_pending);
    assert!(matches!(
        app.hubstaff_status,
        Some(HubstaffStatus::Checking)
    ));
    let elapsed_before_confirmation = engine(app).timer(id).unwrap().elapsed_ms;
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".to_owned()),
        }),
    });
    assert_eq!(state_of(app, id), TimerState::Running);
    assert_eq!(
        engine(app).timer(id).unwrap().elapsed_ms,
        elapsed_before_confirmation,
        "time spent waiting for Hubstaff confirmation is not backdated"
    );
    intent
}

#[test]
fn hubstaff_routine_checks_wait_two_seconds_and_never_overlap() {
    let (_db, mut app) = hubstaff_app("hubstaff-poll-cadence");
    assert!(!app.hubstaff_ready);
    let _ = app.poll_hubstaff_if_due();
    let first_poll = app.hubstaff_poll_id;
    assert!(app.hubstaff_poll_pending);
    let _ = app.poll_hubstaff_if_due();
    assert_eq!(app.hubstaff_poll_id, first_poll);
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: None,
        }),
    );
    assert!(app.hubstaff_ready);
    app.hubstaff_last_poll_started = Some(Instant::now() - Duration::from_secs(1));
    let previous = app.hubstaff_poll_id;
    let _ = app.poll_hubstaff_if_due();
    assert!(!app.hubstaff_poll_pending);
    assert_eq!(app.hubstaff_poll_id, previous);
    app.hubstaff_last_poll_started = Some(Instant::now() - HUBSTAFF_POLL_INTERVAL);
    let _ = app.poll_hubstaff_if_due();
    assert!(app.hubstaff_poll_pending);
    assert_eq!(app.hubstaff_poll_id, previous + 1);
}

#[test]
fn hubstaff_failures_delay_background_retries_but_explicit_starts_are_immediate() {
    let (_db, mut app) = hubstaff_app("hubstaff-poll-backoff");
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: None,
        }),
    );
    assert!(app.hubstaff_ready);
    for failures in 1..=6 {
        let before = Instant::now();
        report_hubstaff(&mut app, None, Err(ProviderError("CLI unavailable".into())));
        assert!(!app.hubstaff_ready);
        assert_eq!(app.hubstaff_poll_failures, failures);
        assert!(app.hubstaff_retry_after.unwrap() >= before + provider::retry_delay(failures));
        app.hubstaff_last_poll_started = Some(Instant::now() - Duration::from_secs(60));
        let previous = app.hubstaff_poll_id;
        let _ = app.poll_hubstaff_if_due();
        assert!(!app.hubstaff_poll_pending);
        assert_eq!(app.hubstaff_poll_id, previous);
    }
    // An expired delay authorizes a single recovery batch.
    app.hubstaff_retry_after = Some(Instant::now() - Duration::from_secs(1));
    let _ = app.poll_hubstaff_if_due();
    assert!(app.hubstaff_poll_pending);
    report_hubstaff(&mut app, None, Err(ProviderError("CLI unavailable".into())));
    assert!(app.hubstaff_retry_after.unwrap() > Instant::now());
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    assert!(app.hubstaff_ready);
    assert_eq!(app.hubstaff_poll_failures, 0);
    assert!(app.hubstaff_retry_after.is_none());
}

#[test]
fn hubstaff_freshness_allows_the_routine_interval_and_query_timeout() {
    let (_db, mut app) = hubstaff_app("hubstaff-poll-freshness");
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: None,
        }),
    );
    app.hubstaff_last_success = Some(Instant::now() - Duration::from_secs(5));
    assert!(!app.expire_hubstaff_state());
    assert!(app.hubstaff_ready);
    app.hubstaff_last_success = Some(Instant::now() - HUBSTAFF_FRESHNESS - Duration::from_secs(1));
    assert!(app.expire_hubstaff_state());
    assert!(!app.hubstaff_ready);
    assert_eq!(state_of(&app, &timer_ids(&app)[0]), TimerState::Running);
}

fn hubstaff_app_with_auto_resume_target(name: &str) -> (TempDb, App, String, String) {
    let (db, mut app) = hubstaff_app(name);
    let timer_id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &timer_id);
    let other_id = app.engine.as_mut().unwrap().add_timer().unwrap();
    let _ = app.update(Message::FocusTimer(other_id.clone()));
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &timer_id), TimerState::Paused);
    assert!(app.hubstaff_resume_target.is_some());
    (db, app, timer_id, other_id)
}

fn hubstaff_app_with_failed_pause(name: &str) -> (TempDb, App, String, rusqlite::Connection) {
    let (db, mut app) = hubstaff_app(name);
    let timer_id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &timer_id);
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_hubstaff_pause BEFORE INSERT ON events
         WHEN NEW.kind = 'pause'
         BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
    )
    .unwrap();
    (db, app, timer_id, conn)
}

/// Pin wall-clock timestamps so review snapshots are reproducible.
fn stabilize_visual_timestamps(app: &mut App) {
    let fixed = at(1_700_000_000_000);
    for summary in &mut app.history {
        summary.session.started_at = fixed;
        summary.session.ended_at = summary.session.ended_at.map(|_| fixed);
        for task in &mut summary.tasks {
            task.created_at = fixed;
            task.updated_at = fixed;
            task.completed_at = task.completed_at.map(|_| fixed);
        }
    }
    for segments in app.breakdowns.values_mut() {
        for segment in segments {
            segment.started_at = fixed;
        }
    }
    for entry in &mut app.selected_log {
        entry.occurred_at = fixed;
    }
}

#[test]
fn hubstaff_start_waits_for_fresh_confirmation_and_status_fits_the_default_window() {
    let (_db, mut app) = hubstaff_app("hubstaff-start");
    let id = timer_ids(&app)[0].clone();

    let start_button_messages = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), SIZE, app.view());
        ui.click("Start")
            .expect("disabled Start remains discoverable");
        ui.into_messages().collect::<Vec<_>>()
    };
    assert!(
        start_button_messages.is_empty(),
        "Start is disabled while Hubstaff readiness is checking"
    );
    let row_toggle_messages = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), SIZE, app.view());
        ui.click(iced_test::selector::id(format!("timer-toggle-{id}")))
            .expect("timer row toggle remains discoverable");
        ui.into_messages().collect::<Vec<_>>()
    };
    assert!(
        row_toggle_messages.is_empty(),
        "timer row Start is disabled while Hubstaff readiness is checking"
    );
    let checking_status = bounds_for(&app, "hubstaff-status");
    assert_text(&app, "Hubstaff · Checking");
    assert!(checking_status.x >= 0.0);
    assert!(checking_status.y >= 0.0);
    assert!(checking_status.x + checking_status.width <= SIZE.0);
    assert!(checking_status.y + checking_status.height <= SIZE.1);
    let checking_primary = bounds_for(&app, "primary-action");

    // A forged Start message also waits for a fresh read-only query.
    let _ = app.update(Message::Start(id.clone()));
    assert!(app.hubstaff_startup_follow.is_none());
    assert!(engine(&app).running().is_none());
    assert!(matches!(
        app.hubstaff_status,
        Some(HubstaffStatus::Checking)
    ));
    let intent = app
        .pending_start
        .as_ref()
        .map(|(_, intent)| *intent)
        .unwrap();
    let request_id = app.hubstaff_poll_id;
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("A very long project name that must not escape the title area".into()),
        }),
    });
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(engine(&app).timer(&id).unwrap().elapsed_ms, 0);
    assert_eq!(checking_primary, bounds_for(&app, "primary-action"));

    for status in [
        HubstaffStatus::Tracking {
            project: Some("A very long project name that must not escape the title area".into()),
        },
        HubstaffStatus::Stopped {
            project: Some("A very long project name that must not escape the title area".into()),
        },
        HubstaffStatus::Unavailable {
            reason: "CLI timed out".into(),
        },
        HubstaffStatus::Checking,
    ] {
        app.hubstaff_status = Some(status);
        app.hubstaff_last_success = Some(Instant::now());
        let badge = bounds_for(&app, "hubstaff-status");
        assert!(badge.x >= 0.0 && badge.x + badge.width <= SIZE.0);
        assert!(badge.y >= PAD_PAGE && badge.y + badge.height <= PAD_PAGE + TITLE_BAR_HEIGHT);
        assert_eq!(checking_primary, bounds_for(&app, "primary-action"));
    }
}

#[test]
fn hubstaff_long_project_snapshots_keep_title_and_four_timer_rows_visible() {
    let (_db, mut app) = hubstaff_app("hubstaff-visual-review");
    let mut ids = timer_ids(&app);
    for _ in 0..3 {
        ids.push(app.engine.as_mut().unwrap().add_timer().unwrap());
    }

    // This is a fixture copied from a Hubstaff status example; no Hubstaff CLI
    // is contacted while the snapshot is rendered.
    let project = "Code Behaviors  [cmu304plh04xm07y9197mgvjs]".to_owned();
    app.hubstaff_status = Some(HubstaffStatus::Tracking {
        project: Some(project.clone()),
    });
    app.hubstaff_last_success = Some(Instant::now());

    let title = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), SIZE, app.view());
        ui.find("LBX Timer").unwrap().bounds()
    };
    let badge = bounds_for(&app, "hubstaff-status");
    let active_panel = bounds_for(&app, "active-panel");
    assert!(
        title.x + title.width + SPACE_S <= badge.x,
        "title and Hubstaff badge overlap: title={title:?}, badge={badge:?}"
    );
    assert!(badge.x + badge.width <= SIZE.0);
    for id in &ids {
        let row = bounds_for(&app, &format!("timer-row-{id}"));
        assert!(row.x >= 0.0 && row.x + row.width <= SIZE.0);
        assert!(row.y >= 0.0 && row.y + row.height <= SIZE.1);
    }
    snapshot(&app, SIZE, "lbx-timer-hubstaff-tracking-four-timers");

    app.hubstaff_status = Some(HubstaffStatus::Stopped {
        project: Some(project),
    });
    app.hubstaff_last_success = Some(Instant::now());
    let stopped_badge = bounds_for(&app, "hubstaff-status");
    assert!(stopped_badge.x + stopped_badge.width <= SIZE.0);
    assert_eq!(bounds_for(&app, "active-panel"), active_panel);
    snapshot(&app, SIZE, "lbx-timer-hubstaff-stopped-four-timers");
}

#[test]
fn hubstaff_stop_pauses_once_and_tracking_return_resumes_that_timer_once() {
    let (_db, mut app) = hubstaff_app("hubstaff-stop");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    let other = app.engine.as_mut().unwrap().add_timer().unwrap();
    let _ = app.update(Message::FocusTimer(other.clone()));
    assert_eq!(app.focused_id.as_deref(), Some(other.as_str()));

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Paused);
    let events = engine(&app).events(&id).unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[1]
        .details
        .as_deref()
        .unwrap()
        .contains("polling and CLI response time can delay detection"));
    assert!(engine(&app)
        .event_log(&id)
        .unwrap()
        .last()
        .unwrap()
        .text
        .contains("Hubstaff reported"));

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(engine(&app).events(&id).unwrap().len(), 2);

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(state_of(&app, &other), TimerState::Unstarted);
    assert_eq!(app.focused_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        engine(&app)
            .events(&id)
            .unwrap()
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        [EventKind::Start, EventKind::Pause, EventKind::Resume]
    );

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(
        engine(&app)
            .events(&id)
            .unwrap()
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        [EventKind::Start, EventKind::Pause, EventKind::Resume]
    );
}

#[test]
fn hubstaff_startup_tracking_starts_the_current_timer_once_from_zero() {
    let (_db, mut app) = hubstaff_app("hubstaff-idle-launch");
    let id = timer_ids(&app)[0].clone();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_text(&app, "Hubstaff · Tracking · Review");
    assert_eq!(
        engine(&app).running().map(|timer| timer.id.as_str()),
        Some(id.as_str())
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(engine(&app).timer(&id).unwrap().elapsed_ms, 0);
    assert_eq!(engine(&app).events(&id).unwrap()[0].kind, EventKind::Start);

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(engine(&app).events(&id).unwrap().len(), 1);

    let second = app.engine.as_mut().unwrap().add_timer().unwrap();
    click_id(&mut app, &format!("timer-row-{second}"));
    assert_eq!(app.focused_id.as_deref(), Some(second.as_str()));
    assert_eq!(
        engine(&app).running().map(|timer| timer.id.as_str()),
        Some(id.as_str())
    );
    assert_eq!(state_of(&app, &second), TimerState::Unstarted);
}

#[test]
fn hubstaff_startup_stopped_waits_then_starts_once_when_tracking_returns() {
    let (_db, mut app) = hubstaff_app("hubstaff-idle-launch-stopped");
    let id = timer_ids(&app)[0].clone();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&app).running().is_none());
    assert_eq!(state_of(&app, &id), TimerState::Unstarted);

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(engine(&app).timer(&id).unwrap().elapsed_ms, 0);
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(engine(&app).events(&id).unwrap().len(), 1);
}

#[test]
fn selecting_timer_two_while_startup_is_stopped_starts_it_when_hubstaff_tracks() {
    let (_db, mut app) = hubstaff_app("hubstaff-startup-select-second");
    let ids = timer_ids(&app);
    let first = ids[0].clone();
    let second = app.engine.as_mut().unwrap().add_timer().unwrap();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert_text(&app, "Hubstaff · Stopped · Review");
    let status = bounds_for(&app, "hubstaff-status");
    let second_row = bounds_for(&app, &format!("timer-row-{second}"));
    assert!(status.x >= 0.0 && status.x + status.width <= SIZE.0);
    assert!(second_row.y + second_row.height <= SIZE.1);
    let toggle_messages = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), SIZE, app.view());
        ui.click(iced_test::selector::id(format!("timer-toggle-{second}")))
            .expect("disabled Timer 2 Start remains discoverable");
        ui.into_messages().collect::<Vec<_>>()
    };
    assert!(toggle_messages.is_empty());

    click_id(&mut app, &format!("timer-row-{second}"));
    assert_eq!(app.focused_id.as_deref(), Some(second.as_str()));
    assert!(engine(&app).running().is_none());
    assert_eq!(state_of(&app, &second), TimerState::Unstarted);

    let session_id = engine(&app).session().id.clone();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(engine(&app).session().id, session_id);
    assert_eq!(state_of(&app, &first), TimerState::Unstarted);
    assert_eq!(state_of(&app, &second), TimerState::Running);
    assert_eq!(engine(&app).events(&second).unwrap().len(), 1);
}

#[test]
fn selecting_current_task_while_stopped_arms_that_timer_for_tracking() {
    let (_db, mut app) = hubstaff_app("hubstaff-select-current-task");
    let first = timer_ids(&app)[0].clone();
    let second = app.engine.as_mut().unwrap().add_timer().unwrap();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );

    let _ = app.update(Message::SelectTask(second.clone()));
    assert_eq!(app.focused_id.as_deref(), Some(second.as_str()));
    assert_eq!(
        app.hubstaff_resume_target.as_ref().unwrap().timer_id,
        second
    );
    assert!(engine(&app).running().is_none());
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &first), TimerState::Unstarted);
    assert_eq!(state_of(&app, &second), TimerState::Running);
}

#[test]
fn selecting_paused_timer_two_while_stopped_replaces_the_resume_target() {
    let (_db, mut app) = hubstaff_app("hubstaff-select-paused-second");
    let first = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &first);
    let second = app.engine.as_mut().unwrap().add_timer().unwrap();
    let _ = app.update(Message::Start(second.clone()));
    let intent = app.pending_start.as_ref().unwrap().1;
    let request_id = app.hubstaff_poll_id;
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    });
    let _ = app.update(Message::Start(first.clone()));
    let intent = app.pending_start.as_ref().unwrap().1;
    let request_id = app.hubstaff_poll_id;
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    });
    let session_id = engine(&app).session().id.clone();

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &first), TimerState::Paused);
    assert_eq!(state_of(&app, &second), TimerState::Paused);
    assert_eq!(app.hubstaff_resume_target.as_ref().unwrap().timer_id, first);

    let second_row = bounds_for(&app, &format!("timer-row-{second}"));
    assert!(second_row.y >= 0.0 && second_row.y + second_row.height <= SIZE.1);
    click_id(&mut app, &format!("timer-row-{second}"));
    assert!(engine(&app).running().is_none());
    assert_eq!(
        app.hubstaff_resume_target.as_ref().unwrap().timer_id,
        second
    );

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(engine(&app).session().id, session_id);
    assert_eq!(state_of(&app, &first), TimerState::Paused);
    assert_eq!(state_of(&app, &second), TimerState::Running);
    assert_eq!(
        engine(&app)
            .events(&second)
            .unwrap()
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        [EventKind::Start, EventKind::Pause, EventKind::Resume]
    );
}

#[test]
fn selection_under_checking_unavailable_or_stale_status_does_not_arm_auto_start() {
    for (name, status, updated) in [
        ("checking", HubstaffStatus::Checking, None),
        (
            "unavailable",
            HubstaffStatus::Unavailable {
                reason: "CLI unavailable".into(),
            },
            None,
        ),
        (
            "stale",
            HubstaffStatus::Stopped {
                project: Some("Review".into()),
            },
            Some(Instant::now() - HUBSTAFF_FRESHNESS - Duration::from_secs(1)),
        ),
    ] {
        let (_db, mut app) = hubstaff_app(&format!("hubstaff-select-{name}"));
        let second = app.engine.as_mut().unwrap().add_timer().unwrap();
        app.hubstaff_status = Some(status);
        app.hubstaff_last_success = updated;
        click_id(&mut app, &format!("timer-row-{second}"));
        assert!(app.hubstaff_resume_target.is_none());
        report_hubstaff(
            &mut app,
            None,
            Ok(HubstaffSnapshot {
                tracking: true,
                project: Some("Review".into()),
            }),
        );
        assert!(engine(&app).running().is_none());
        assert_eq!(state_of(&app, &second), TimerState::Unstarted);
    }
}

#[test]
fn hubstaff_startup_intent_survives_readiness_failure_until_fresh_tracking() {
    let (_db, mut app) = hubstaff_app("hubstaff-startup-readiness-error");
    let id = timer_ids(&app)[0].clone();
    report_hubstaff(&mut app, None, Err(ProviderError("not ready yet".into())));
    assert_text(&app, "Hubstaff · Unavailable");
    assert!(engine(&app).running().is_none());
    assert_eq!(state_of(&app, &id), TimerState::Unstarted);

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(engine(&app).timer(&id).unwrap().elapsed_ms, 0);
}

#[test]
fn local_pause_end_and_close_cancel_pending_hubstaff_startup_follow() {
    let (_pause_db, mut paused) = hubstaff_app("hubstaff-startup-pause-cancel");
    let _ = paused.update(Message::Pause);
    let pause_timer = timer_ids(&paused)[0].clone();
    report_hubstaff(
        &mut paused,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&paused).running().is_none());
    assert_eq!(state_of(&paused, &pause_timer), TimerState::Unstarted);

    let (_end_db, mut ended) = hubstaff_app("hubstaff-startup-end-cancel");
    let _ = ended.update(Message::EndSession);
    let ended_timer = timer_ids(&ended)[0].clone();
    report_hubstaff(
        &mut ended,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&ended).running().is_none());
    assert_eq!(state_of(&ended, &ended_timer), TimerState::Unstarted);

    let (_close_db, mut closed) = hubstaff_app("hubstaff-startup-close-cancel");
    let _ = closed.update(Message::CloseRequested(iced::window::Id::unique()));
    let close_timer = timer_ids(&closed)[0].clone();
    report_hubstaff(
        &mut closed,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&closed).running().is_none());
    assert_eq!(state_of(&closed, &close_timer), TimerState::Unstarted);
}

#[test]
fn hubstaff_startup_does_not_adopt_a_task_from_history_or_start_in_standalone_mode() {
    let db = TempDb::new("hubstaff-startup-history");
    let (old_session, [old_task, ..]) = seed_linked_session(&db.0);
    let mut app = App::at_path_with_observer(
        db.0.clone(),
        ObserverConfig::Hubstaff(std::path::PathBuf::from("/fake/HubstaffCLI.bin.x86_64")),
    );
    let current_session = engine(&app).session().id.clone();
    let current_timer = timer_ids(&app)[0].clone();
    assert_ne!(current_session, old_session);
    assert_ne!(current_timer, old_task);
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(
        engine(&app).running().map(|timer| timer.id.as_str()),
        Some(current_timer.as_str())
    );
    assert_eq!(
        engine(&app)
            .timer(&current_timer)
            .unwrap()
            .continues_task_id,
        None
    );
    assert!(engine(&app).task(&old_task).unwrap().is_some());

    let standalone_db = TempDb::new("standalone-does-not-auto-start");
    let mut standalone = App::at_path(standalone_db.0.clone());
    let standalone_timer = timer_ids(&standalone)[0].clone();
    let _ = standalone.update(Message::Tick);
    assert!(engine(&standalone).running().is_none());
    assert_eq!(
        state_of(&standalone, &standalone_timer),
        TimerState::Unstarted
    );
}

#[test]
fn failed_hubstaff_startup_auto_start_is_consumed_until_explicit_retry() {
    let (db, mut app) = hubstaff_app("hubstaff-startup-start-error");
    let id = timer_ids(&app)[0].clone();
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_hubstaff_start BEFORE INSERT ON events
         WHEN NEW.kind = 'start'
         BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
    )
    .unwrap();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&app).running().is_none());
    assert!(app.error_message.is_some());
    assert!(engine(&app).events(&id).unwrap().is_empty());

    conn.execute_batch("DROP TRIGGER reject_hubstaff_start")
        .unwrap();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&app).running().is_none());
    assert!(engine(&app).events(&id).unwrap().is_empty());

    let _ = app.update(Message::Start(id.clone()));
    let intent = app.pending_start.as_ref().unwrap().1;
    let request_id = app.hubstaff_poll_id;
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    });
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(engine(&app).events(&id).unwrap().len(), 1);
}

#[test]
fn hubstaff_unavailable_and_stale_states_preserve_an_armed_resume() {
    let (_unavailable_db, mut unavailable, unavailable_id, _) =
        hubstaff_app_with_auto_resume_target("hubstaff-auto-unavailable");
    report_hubstaff(
        &mut unavailable,
        None,
        Err(ProviderError("CLI unavailable".into())),
    );
    assert!(unavailable.hubstaff_resume_target.is_some());
    report_hubstaff(
        &mut unavailable,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&unavailable, &unavailable_id), TimerState::Running);

    let (_stale_db, mut stale, stale_id, _) =
        hubstaff_app_with_auto_resume_target("hubstaff-auto-stale");
    stale.hubstaff_last_success =
        Some(Instant::now() - HUBSTAFF_FRESHNESS - Duration::from_secs(1));
    let _ = stale.update(Message::Tick);
    assert!(stale.hubstaff_resume_target.is_some());
    report_hubstaff(
        &mut stale,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&stale, &stale_id), TimerState::Running);
}

#[test]
fn hubstaff_failed_automatic_resume_is_consumed_until_the_user_retries() {
    let (db, mut app, id, _) = hubstaff_app_with_auto_resume_target("hubstaff-auto-resume-error");
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_hubstaff_resume BEFORE INSERT ON events
         WHEN NEW.kind = 'resume'
         BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
    )
    .unwrap();

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Paused);
    assert!(app.hubstaff_resume_target.is_none());
    assert!(app.error_message.is_some());
    assert_eq!(engine(&app).events(&id).unwrap().len(), 2);

    conn.execute_batch("DROP TRIGGER reject_hubstaff_resume")
        .unwrap();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Paused);
    assert_eq!(engine(&app).events(&id).unwrap().len(), 2);

    let _ = app.update(Message::Start(id.clone()));
    let intent = app.pending_start.as_ref().unwrap().1;
    let request_id = app.hubstaff_poll_id;
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    });
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(engine(&app).events(&id).unwrap().len(), 3);
}

#[test]
fn hubstaff_manual_actions_cancel_the_armed_resume_target() {
    let (_pause_db, mut paused, paused_id, _) =
        hubstaff_app_with_auto_resume_target("hubstaff-manual-pause");
    let _ = paused.update(Message::Pause);
    assert!(paused.hubstaff_resume_target.is_none());
    report_hubstaff(
        &mut paused,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&paused, &paused_id), TimerState::Paused);

    let (_end_db, mut ended, _, _) = hubstaff_app_with_auto_resume_target("hubstaff-manual-end");
    let _ = ended.update(Message::EndSession);
    report_hubstaff(
        &mut ended,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&ended).running().is_none());

    let (_close_db, mut closed, _, _) =
        hubstaff_app_with_auto_resume_target("hubstaff-manual-close");
    let _ = closed.update(Message::CloseRequested(iced::window::Id::unique()));
    report_hubstaff(
        &mut closed,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&closed).running().is_none());
}

#[test]
fn hubstaff_end_and_close_cancel_auto_resume_when_pause_save_fails() {
    let actions = [
        ("end", Message::EndSession),
        ("close", Message::CloseRequested(iced::window::Id::unique())),
    ];
    for (name, action) in actions {
        let (db, mut app) = hubstaff_app(&format!("hubstaff-pending-{name}"));
        let timer_id = timer_ids(&app)[0].clone();
        start_hubstaff_timer(&mut app, &timer_id);
        let conn = rusqlite::Connection::open(&db.0).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_hubstaff_pause BEFORE INSERT ON events
             WHEN NEW.kind = 'pause'
             BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
        )
        .unwrap();
        report_hubstaff(
            &mut app,
            None,
            Ok(HubstaffSnapshot {
                tracking: false,
                project: Some("Review".into()),
            }),
        );
        assert!(app.hubstaff_pause_retry);
        assert!(app.hubstaff_resume_candidate.is_some());

        let _ = app.update(action);
        assert!(app.hubstaff_resume_candidate.is_none());
        assert!(app.hubstaff_resume_target.is_none());
        assert!(app.hubstaff_pause_retry);
        conn.execute_batch("DROP TRIGGER reject_hubstaff_pause")
            .unwrap();
        let _ = app.update(Message::Tick);
        assert!(!app.hubstaff_pause_retry);
        assert_eq!(state_of(&app, &timer_id), TimerState::Paused);
        report_hubstaff(
            &mut app,
            None,
            Ok(HubstaffSnapshot {
                tracking: true,
                project: Some("Review".into()),
            }),
        );
        assert!(engine(&app).running().is_none());
        assert_eq!(state_of(&app, &timer_id), TimerState::Paused);
    }
}

#[test]
fn hubstaff_explicit_start_wins_and_focus_selects_next_stopped_target() {
    let (_start_db, mut started, paused_id, other_id) =
        hubstaff_app_with_auto_resume_target("hubstaff-manual-start");
    let _ = started.update(Message::Start(other_id.clone()));
    assert!(started.hubstaff_resume_target.is_none());
    let intent = started.pending_start.as_ref().unwrap().1;
    let request_id = started.hubstaff_poll_id;
    let _ = started.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    });
    assert_eq!(state_of(&started, &paused_id), TimerState::Paused);
    assert_eq!(state_of(&started, &other_id), TimerState::Running);

    let (_focus_db, mut focused, paused_id, other_id) =
        hubstaff_app_with_auto_resume_target("hubstaff-focus-selects-target");
    let row = bounds_for(&focused, &format!("timer-row-{other_id}"));
    assert!(row.x >= 0.0 && row.x + row.width <= SIZE.0);
    let _ = focused.update(Message::FocusTimer(other_id.clone()));
    assert_eq!(
        focused.hubstaff_resume_target.as_ref().unwrap().timer_id,
        other_id
    );
    report_hubstaff(
        &mut focused,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&focused, &paused_id), TimerState::Paused);
    assert_eq!(state_of(&focused, &other_id), TimerState::Running);
}

#[test]
fn hubstaff_task_selection_and_session_load_cancel_the_armed_resume_target() {
    let select_db = TempDb::new("hubstaff-select-cancels");
    let (_, [history_task, ..]) = seed_linked_session(&select_db.0);
    let mut selected = App::at_path_with_observer(
        select_db.0.clone(),
        ObserverConfig::Hubstaff(std::path::PathBuf::from("/fake/HubstaffCLI.bin.x86_64")),
    );
    let selected_timer = timer_ids(&selected)[0].clone();
    start_hubstaff_timer(&mut selected, &selected_timer);
    report_hubstaff(
        &mut selected,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(selected.hubstaff_resume_target.is_some());
    let _ = selected.update(Message::SelectTask(history_task));
    assert!(selected.hubstaff_resume_target.is_none());
    report_hubstaff(
        &mut selected,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&selected, &selected_timer), TimerState::Paused);
    assert!(engine(&selected).running().is_none());

    let load_db = TempDb::new("hubstaff-load-cancels");
    let (source_session, _) = seed_linked_session(&load_db.0);
    let mut loaded = App::at_path_with_observer(
        load_db.0.clone(),
        ObserverConfig::Hubstaff(std::path::PathBuf::from("/fake/HubstaffCLI.bin.x86_64")),
    );
    let loaded_timer = timer_ids(&loaded)[0].clone();
    start_hubstaff_timer(&mut loaded, &loaded_timer);
    report_hubstaff(
        &mut loaded,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(loaded.hubstaff_resume_target.is_some());
    loaded.load_into_session(source_session, LoadMode::AddToCurrent);
    assert!(loaded.hubstaff_resume_target.is_none());
    report_hubstaff(
        &mut loaded,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&loaded, &loaded_timer), TimerState::Paused);
    assert!(engine(&loaded).running().is_none());
}

#[test]
fn hubstaff_session_load_cancels_auto_resume_even_when_pause_save_fails() {
    let db = TempDb::new("hubstaff-load-pause-failure");
    let (source_session, _) = seed_linked_session(&db.0);
    let mut app = App::at_path_with_observer(
        db.0.clone(),
        ObserverConfig::Hubstaff(std::path::PathBuf::from("/fake/HubstaffCLI.bin.x86_64")),
    );
    let timer_id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &timer_id);
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_hubstaff_pause BEFORE INSERT ON events
         WHEN NEW.kind = 'pause'
         BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
    )
    .unwrap();

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(app.hubstaff_pause_retry);
    assert!(app.hubstaff_resume_candidate.is_some());

    app.load_into_session(source_session, LoadMode::AddToCurrent);
    assert!(app.hubstaff_resume_candidate.is_none());
    assert!(app.hubstaff_resume_target.is_none());
    assert!(app.hubstaff_pause_retry);
    conn.execute_batch("DROP TRIGGER reject_hubstaff_pause")
        .unwrap();
    let _ = app.update(Message::Tick);
    assert!(!app.hubstaff_pause_retry);
    assert_eq!(state_of(&app, &timer_id), TimerState::Paused);

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&app).running().is_none());
    assert_eq!(state_of(&app, &timer_id), TimerState::Paused);
}

#[test]
fn hubstaff_failure_and_stale_state_keep_counting_without_pause_resume_events() {
    let (_db, mut app) = hubstaff_app("hubstaff-failure");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);

    report_hubstaff(&mut app, None, Err(ProviderError("CLI timed out".into())));
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(!app.can_start_timer());
    assert!(matches!(
        app.hubstaff_status,
        Some(HubstaffStatus::Unavailable { .. })
    ));
    let paused_elapsed = engine(&app).timer(&id).unwrap().elapsed_ms;
    app.monotonic_start = Instant::now() - Duration::from_secs(120);

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: None,
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(engine(&app).timer(&id).unwrap().elapsed_ms >= paused_elapsed + 119_000);
    assert_eq!(engine(&app).events(&id).unwrap().len(), 1);
    assert_eq!(app.time_verifications.len(), 1);
    assert!(app.time_verifications[0].ended_at.is_some());
    assert!(app.can_start_timer());
    app.hubstaff_last_success = Some(Instant::now() - HUBSTAFF_FRESHNESS - Duration::from_secs(1));
    let _ = app.update(Message::Tick);
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(!app.can_start_timer());
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(app.can_start_timer());
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: None,
        }),
    );
    assert_eq!(engine(&app).events(&id).unwrap().len(), 1);
    assert_eq!(app.time_verifications.len(), 2);
}

#[test]
fn hubstaff_stale_state_marks_a_running_timer_unverified_until_audit_confirmation() {
    let (_db, mut app) = hubstaff_app("hubstaff-stale-running");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    app.hubstaff_last_success = Some(Instant::now() - HUBSTAFF_FRESHNESS - Duration::from_secs(1));

    let _ = app.update(Message::Tick);
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(!app.can_start_timer());
    let events = engine(&app).events(&id).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(app.verification_label(Some(&id)), Some("Unverified"));
    assert!(app.time_verifications[0]
        .reason
        .contains("Hubstaff status is stale"));

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(app.can_start_timer());
}

#[test]
fn hubstaff_connection_recovery_respects_local_cancellation_and_system_sleep() {
    for action in [
        Message::Pause,
        Message::EndSession,
        Message::CloseRequested(iced::window::Id::unique()),
    ] {
        let (_db, mut app) = hubstaff_app("hubstaff-recovery-cancel");
        let id = timer_ids(&app)[0].clone();
        start_hubstaff_timer(&mut app, &id);
        report_hubstaff(&mut app, None, Err(ProviderError("CLI timed out".into())));
        assert!(app.hubstaff_resume_target.is_none());
        assert_eq!(state_of(&app, &id), TimerState::Running);
        let _ = app.update(action);
        report_hubstaff(&mut app, None, Err(ProviderError("invalid JSON".into())));
        report_hubstaff(
            &mut app,
            None,
            Ok(HubstaffSnapshot {
                tracking: true,
                project: None,
            }),
        );
        assert!(engine(&app).running().is_none());
        assert!(app.hubstaff_resume_target.is_none());
    }

    let (_db, mut app) = hubstaff_app("hubstaff-stale-after-sleep");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    let mono = app.mono_ms();
    app.engine
        .as_mut()
        .unwrap()
        .tick(Utc::now() - chrono::Duration::seconds(10), mono)
        .unwrap();
    app.hubstaff_last_success = Some(Instant::now() - HUBSTAFF_FRESHNESS - Duration::from_secs(1));
    let _ = app.update(Message::Tick);
    assert_eq!(
        engine(&app).events(&id).unwrap().last().unwrap().kind,
        EventKind::SleepPause
    );
    assert!(app.hubstaff_resume_target.is_none());
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: None,
        }),
    );
    assert!(engine(&app).running().is_none());
}

#[test]
fn details_shows_manual_and_hubstaff_event_triggers_with_stable_geometry() {
    let (_db, mut app) = hubstaff_app("hubstaff-event-triggers");
    let id = timer_ids(&app)[0].clone();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: None,
        }),
    );
    click_text(&mut app, "Details");
    let card = bounds_for(&app, "active-panel");
    let details = bounds_for(&app, "details-panel");
    assert_text(
        &app,
        "Started “Timer 1” — Triggered by Hubstaff: tracking confirmed at launch.",
    );

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: None,
        }),
    );
    assert!(app
        .selected_log
        .last()
        .unwrap()
        .text
        .contains("Triggered by Hubstaff. Hubstaff reported tracking stopped"));
    // Repeated stopped responses retain the same target and pause event.
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: None,
        }),
    );
    assert_eq!(engine(&app).events(&id).unwrap().len(), 2);
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: None,
        }),
    );
    assert_text(
        &app,
        "Resumed “Timer 1” — Triggered by Hubstaff: tracking confirmed after an automatic pause.",
    );
    click_text(&mut app, "Pause");
    assert_text(&app, "Paused “Timer 1” — Triggered manually in LBX.");
    click_text(&mut app, "Resume");
    let intent = app.pending_start.as_ref().unwrap().1;
    report_hubstaff(
        &mut app,
        Some(intent),
        Ok(HubstaffSnapshot {
            tracking: true,
            project: None,
        }),
    );
    assert_text(&app, "Resumed “Timer 1” — Triggered manually in LBX.");
    assert_eq!(bounds_for(&app, "active-panel"), card);
    assert_eq!(bounds_for(&app, "details-panel"), details);
    assert!(!app.history_expanded);
}

#[test]
fn connection_outage_keeps_counting_then_automatically_clears_unverified_with_audit_evidence() {
    let (_db, mut app) = hubstaff_app("hubstaff-unverified-confirmed");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    for _ in 0..3 {
        click_text(&mut app, "+ Add timer");
    }
    let card = bounds_for(&app, "active-panel");
    let first_row = bounds_for(&app, &format!("timer-row-{id}"));
    report_hubstaff(&mut app, None, Err(ProviderError("CLI timeout".into())));
    assert_text(&app, "RUNNING · UNVERIFIED");
    assert_text(&app, "Hubstaff · Unverified");
    let badge = bounds_for(&app, "hubstaff-status");
    assert!(badge.x >= 0.0 && badge.x + badge.width <= SIZE.0);
    for timer_id in timer_ids(&app) {
        let row = bounds_for(&app, &format!("timer-row-{timer_id}"));
        assert!(row.y >= 0.0 && row.y + row.height <= SIZE.1);
    }
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(bounds_for(&app, "active-panel"), card);
    assert_eq!(bounds_for(&app, &format!("timer-row-{id}")), first_row);
    let before = engine(&app).timer(&id).unwrap().elapsed_ms;
    app.monotonic_start -= Duration::from_millis(500);
    let _ = app.update(Message::Tick);
    assert!(engine(&app).timer(&id).unwrap().elapsed_ms >= before + 450);
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(
        app.verification_label(Some(&id)),
        Some("Unverified"),
        "a tracking reply alone cannot establish continuity"
    );
    let audit = std::env::temp_dir().join(format!("lbx-audit-ui-{}", uuid::Uuid::new_v4()));
    let anchor = Utc::now() - chrono::Duration::seconds(60);
    std::fs::write(
        &audit,
        format!(
            "{} [AUDIT] (START_TRACKING) Tracking Started\n",
            anchor.format("%Y-%m-%d %H:%M:%S%.6f%z")
        ),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let results = runtime.block_on(verification::check_audit(
        Some(audit.clone()),
        app.time_verifications.clone(),
        Utc::now(),
        true,
    ));
    assert_eq!(results[0].state, VerificationState::Confirmed);
    let _ = app.update(Message::HubstaffVerificationChecked(results));
    assert_eq!(app.verification_label(Some(&id)), None);
    assert_text(&app, "RUNNING");
    assert_text(&app, "Hubstaff · Tracking · Review");
    assert_eq!(engine(&app).events(&id).unwrap().len(), 1);
    click_text(&mut app, "Details");
    let confirmed = app
        .selected_log
        .iter()
        .find(|entry| entry.text.contains("Confirmed time"))
        .unwrap()
        .text
        .clone();
    assert_text(&app, &confirmed);
    assert_eq!(bounds_for(&app, "active-panel"), card);
    assert!(!app.history_expanded);
    std::fs::remove_file(audit).unwrap();
}

#[test]
fn resuming_an_adjusted_timer_uses_original_local_time_for_the_next_verification() {
    let (_db, mut app) = hubstaff_app("hubstaff-adjusted-resume-checkpoint");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    app.monotonic_start -= Duration::from_millis(500);
    app.hubstaff_verified_checkpoint.as_mut().unwrap().1 -= chrono::Duration::milliseconds(500);
    report_hubstaff(&mut app, None, Err(ProviderError("timeout".into())));
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: None,
        }),
    );
    let interval = app.time_verifications[0].clone();
    let _ = app.update(Message::HubstaffVerificationChecked(vec![
        VerificationResult {
            id: interval.id,
            ended_at: interval.ended_at.unwrap(),
            state: VerificationState::Adjusted,
            evidence: "Automatically adjusted".into(),
            excluded_ranges: vec![(interval.started_at, interval.ended_at.unwrap())],
        },
    ]));
    assert!(
        engine(&app).timer(&id).unwrap().raw_elapsed_ms()
            > engine(&app).timer(&id).unwrap().elapsed_ms
    );
    click_text(&mut app, "Pause");
    start_hubstaff_timer(&mut app, &id);
    assert_eq!(
        app.hubstaff_verified_checkpoint.as_ref().unwrap().2,
        engine(&app).timer(&id).unwrap().raw_elapsed_ms()
    );
}

#[test]
fn hubstaff_failed_pause_stays_gated_and_retries_until_it_is_saved() {
    let (db, mut app) = hubstaff_app("hubstaff-pause-retry");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_hubstaff_pause BEFORE INSERT ON events
         WHEN NEW.kind = 'pause'
         BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
    )
    .unwrap();

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(app.hubstaff_pause_retry);
    assert!(!app.can_start_timer());
    assert!(app.error_message.is_some());

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(app.hubstaff_pause_retry);
    assert_eq!(engine(&app).events(&id).unwrap().len(), 1);

    let _ = app.update(Message::Tick);
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert!(app.hubstaff_pause_retry);

    conn.execute_batch("DROP TRIGGER reject_hubstaff_pause")
        .unwrap();
    let _ = app.update(Message::Tick);
    assert_eq!(state_of(&app, &id), TimerState::Paused);
    assert!(!app.hubstaff_pause_retry);
    assert!(app.error_message.is_none());
    assert_eq!(engine(&app).events(&id).unwrap().len(), 2);

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &id), TimerState::Running);
    assert_eq!(engine(&app).events(&id).unwrap().len(), 3);
}

#[test]
fn selected_timer_target_survives_pause_retry_and_needs_a_fresh_tracking_poll() {
    let (_db, mut app, first, conn) =
        hubstaff_app_with_failed_pause("hubstaff-select-during-retry");
    let second = app.engine.as_mut().unwrap().add_timer().unwrap();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(app.hubstaff_pause_retry);
    assert_eq!(
        app.hubstaff_resume_candidate.as_ref().unwrap().timer_id,
        first
    );

    click_id(&mut app, &format!("timer-row-{second}"));
    assert!(app.hubstaff_resume_candidate.is_none());
    assert_eq!(
        app.hubstaff_resume_target.as_ref().unwrap().timer_id,
        second
    );
    assert_eq!(state_of(&app, &second), TimerState::Unstarted);

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(app.hubstaff_pause_retry);
    assert!(app.hubstaff_resume_candidate.is_none());
    assert_eq!(
        app.hubstaff_resume_target.as_ref().unwrap().timer_id,
        second
    );
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(app.hubstaff_pause_retry);
    assert!(engine(&app).running().is_some());
    assert_eq!(state_of(&app, &second), TimerState::Unstarted);

    conn.execute_batch("DROP TRIGGER reject_hubstaff_pause")
        .unwrap();
    let _ = app.update(Message::Tick);
    assert!(!app.hubstaff_pause_retry);
    assert_eq!(state_of(&app, &first), TimerState::Paused);
    assert_eq!(
        app.hubstaff_resume_target.as_ref().unwrap().timer_id,
        second
    );
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &second), TimerState::Running);
    assert_eq!(engine(&app).events(&second).unwrap().len(), 1);
}

#[test]
fn hubstaff_user_end_does_not_rearm_auto_resume_during_a_failed_pause() {
    let (_db, mut app, timer_id, conn) = hubstaff_app_with_failed_pause("hubstaff-end-retry-arm");
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(app.hubstaff_pause_retry);
    assert!(app.hubstaff_resume_candidate.is_some());

    let _ = app.update(Message::EndSession);
    assert!(app.hubstaff_resume_candidate.is_none());
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(app.hubstaff_pause_retry);
    assert!(app.hubstaff_resume_candidate.is_none());

    conn.execute_batch("DROP TRIGGER reject_hubstaff_pause")
        .unwrap();
    let _ = app.update(Message::Tick);
    assert!(!app.hubstaff_pause_retry);
    assert!(app.hubstaff_resume_target.is_none());
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &timer_id), TimerState::Paused);
    assert!(engine(&app).running().is_none());
}

#[test]
fn hubstaff_unavailable_pause_failure_retains_target_through_a_stopped_reply() {
    let (_db, mut app, timer_id, conn) =
        hubstaff_app_with_failed_pause("hubstaff-unavailable-retry-arm");
    report_hubstaff(&mut app, None, Err(ProviderError("CLI unavailable".into())));
    assert!(!app.hubstaff_pause_retry);
    assert_eq!(state_of(&app, &timer_id), TimerState::Running);
    assert_eq!(app.verification_label(Some(&timer_id)), Some("Unverified"));

    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    assert!(app.hubstaff_pause_retry);
    assert!(app.hubstaff_resume_candidate.is_some());

    conn.execute_batch("DROP TRIGGER reject_hubstaff_pause")
        .unwrap();
    let _ = app.update(Message::Tick);
    assert!(!app.hubstaff_pause_retry);
    assert!(app.hubstaff_resume_target.is_some());
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert_eq!(state_of(&app, &timer_id), TimerState::Running);
}

#[test]
fn hubstaff_pause_retry_freezes_detection_time_across_retries_and_user_actions() {
    let (db, mut app) = hubstaff_app("hubstaff-pause-boundary");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    let session_id = engine(&app).session().id.clone();
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_hubstaff_pause BEFORE INSERT ON events
         WHEN NEW.kind = 'pause'
         BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
    )
    .unwrap();

    app.monotonic_start = Instant::now() - Duration::from_secs(5);
    let detected_before = Utc::now();
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: false,
            project: Some("Review".into()),
        }),
    );
    let detected_after = Utc::now();
    let detected_elapsed = engine(&app).timer(&id).unwrap().elapsed_ms;
    assert!((4_000..=6_000).contains(&detected_elapsed));
    assert_eq!(engine(&app).events(&id).unwrap().len(), 1);

    app.monotonic_start = Instant::now() - Duration::from_secs(15);
    let _ = app.update(Message::Tick);
    assert_eq!(
        engine(&app).timer(&id).unwrap().elapsed_ms,
        detected_elapsed
    );

    app.monotonic_start = Instant::now() - Duration::from_secs(20);
    let _ = app.update(Message::Pause);
    assert!(app.hubstaff_resume_candidate.is_none());
    assert_eq!(
        engine(&app).timer(&id).unwrap().elapsed_ms,
        detected_elapsed
    );
    assert_eq!(engine(&app).session().id, session_id);

    app.monotonic_start = Instant::now() - Duration::from_secs(25);
    let _ = app.update(Message::EndSession);
    assert_eq!(
        engine(&app).timer(&id).unwrap().elapsed_ms,
        detected_elapsed
    );
    assert_eq!(engine(&app).session().id, session_id);

    app.monotonic_start = Instant::now() - Duration::from_secs(30);
    let _ = app.update(Message::CloseRequested(iced::window::Id::unique()));
    assert_eq!(
        engine(&app).timer(&id).unwrap().elapsed_ms,
        detected_elapsed
    );
    assert_eq!(engine(&app).session().id, session_id);

    conn.execute_batch("DROP TRIGGER reject_hubstaff_pause")
        .unwrap();
    app.monotonic_start = Instant::now() - Duration::from_secs(40);
    let _ = app.update(Message::EndSession);
    let events = engine(&app).events(&id).unwrap();
    assert_eq!(events[1].kind, EventKind::Pause);
    assert_eq!(events[1].elapsed_ms, detected_elapsed);
    assert!(events[1].occurred_at.timestamp_millis() >= detected_before.timestamp_millis());
    assert!(events[1].occurred_at.timestamp_millis() <= detected_after.timestamp_millis());
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].kind, EventKind::Stop);
    report_hubstaff(
        &mut app,
        None,
        Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    );
    assert!(engine(&app).running().is_none());
}

#[test]
fn hubstaff_end_session_keeps_tracking_independent_and_pause_cancels_a_pending_start() {
    let (_db, mut app) = hubstaff_app("hubstaff-end");
    let first = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &first);

    let _ = app.update(Message::EndSession);
    assert!(engine(&app).running().is_none());
    assert_eq!(engine(&app).timers().len(), 1);
    assert!(matches!(
        app.hubstaff_status,
        Some(HubstaffStatus::Tracking { .. })
    ));

    let second = timer_ids(&app)[0].clone();
    let _ = app.update(Message::Start(second.clone()));
    let intent = app.pending_start.as_ref().unwrap().1;
    let request_id = app.hubstaff_poll_id;
    let _ = app.update(Message::Pause);
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Still tracking".into()),
        }),
    });
    assert!(engine(&app).running().is_none());
    assert!(app.pending_start.is_none());

    let _ = app.update(Message::Start(second.clone()));
    let intent = app.pending_start.as_ref().unwrap().1;
    let request_id = app.hubstaff_poll_id;
    let _ = app.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Still tracking".into()),
        }),
    });
    assert_eq!(state_of(&app, &second), TimerState::Running);
}

#[test]
fn hubstaff_end_session_and_close_cancel_pending_async_starts() {
    let (_end_db, mut ended) = hubstaff_app("hubstaff-end-pending");
    let old_id = timer_ids(&ended)[0].clone();
    let _ = ended.update(Message::Start(old_id));
    let intent = ended.pending_start.as_ref().unwrap().1;
    let request_id = ended.hubstaff_poll_id;
    let _ = ended.update(Message::EndSession);
    let _ = ended.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Still tracking".into()),
        }),
    });
    assert!(engine(&ended).running().is_none());
    assert!(ended.pending_start.is_none());

    let (_close_db, mut closed) = hubstaff_app("hubstaff-close-pending");
    let old_id = timer_ids(&closed)[0].clone();
    let _ = closed.update(Message::Start(old_id));
    let intent = closed.pending_start.as_ref().unwrap().1;
    let request_id = closed.hubstaff_poll_id;
    let _ = closed.update(Message::CloseRequested(iced::window::Id::unique()));
    let _ = closed.update(Message::HubstaffPolled {
        request_id,
        start_intent: Some(intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Still tracking".into()),
        }),
    });
    assert!(engine(&closed).running().is_none());
    assert!(closed.pending_start.is_none());
}

#[test]
fn hubstaff_newer_start_intent_waits_for_its_own_fresh_poll() {
    let (_db, mut app) = hubstaff_app("hubstaff-newer-intent");
    let first = timer_ids(&app)[0].clone();
    let second = app.engine.as_mut().unwrap().add_timer().unwrap();

    let _ = app.update(Message::Start(first.clone()));
    let old_intent = app.pending_start.as_ref().unwrap().1;
    let old_request = app.hubstaff_poll_id;
    let _ = app.update(Message::Start(second.clone()));
    let new_intent = app.pending_start.as_ref().unwrap().1;
    assert_ne!(old_intent, new_intent);
    assert_eq!(app.hubstaff_poll_id, old_request, "polls never overlap");

    let _ = app.update(Message::HubstaffPolled {
        request_id: old_request,
        start_intent: Some(old_intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    });
    assert!(engine(&app).running().is_none());
    assert_eq!(app.hubstaff_poll_id, old_request + 1);
    let new_request = app.hubstaff_poll_id;
    let _ = app.update(Message::HubstaffPolled {
        request_id: new_request,
        start_intent: Some(new_intent),
        result: Ok(HubstaffSnapshot {
            tracking: true,
            project: Some("Review".into()),
        }),
    });
    assert_eq!(state_of(&app, &first), TimerState::Unstarted);
    assert_eq!(state_of(&app, &second), TimerState::Running);
}

#[test]
fn hubstaff_unavailable_never_starts_from_a_forged_message_but_keeps_local_controls() {
    let db = TempDb::new("hubstaff-unavailable-controls");
    let mut app = App::at_path_with_observer(
        db.0.clone(),
        ObserverConfig::Unavailable("bad explicit CLI override".into()),
    );
    let id = timer_ids(&app)[0].clone();
    let _ = app.update(Message::Start(id.clone()));
    assert!(engine(&app).running().is_none());
    assert!(app.pending_start.is_none());

    assert!(app.run(|engine, now, mono| engine.start(&id, now, mono)));
    assert_eq!(state_of(&app, &id), TimerState::Running);
    click_text(&mut app, "Pause");
    assert_eq!(state_of(&app, &id), TimerState::Paused);
    let session_before = engine(&app).session().id.clone();
    click_text(&mut app, "End session");
    assert_ne!(engine(&app).session().id, session_before);
    assert!(matches!(
        app.hubstaff_status,
        Some(HubstaffStatus::Unavailable { .. })
    ));
}

/// Record a finished three-timer session with injected clocks:
/// Review 5 min, Email 3 min, Review 2 min, idle 1 min, QA 1 min.
fn seed_linked_session(path: &std::path::Path) -> (String, [String; 3]) {
    let mut engine =
        TimerEngine::boot(AppData::open(path).unwrap(), at(0), 0).expect("seed engine");
    let review = engine.timers()[0].id.clone();
    let email = engine.add_timer().unwrap();
    let qa = engine.add_timer().unwrap();
    engine.update_name_notes(&review, "Review", "").unwrap();
    engine.update_name_notes(&email, "Email", "").unwrap();
    let minute = 60_000i64;
    let step = |engine: &mut TimerEngine, t: i64| engine.tick(at(t), t as u64).unwrap();
    engine.start(&review, at(0), 0).unwrap();
    step(&mut engine, 5 * minute);
    engine
        .start(&email, at(5 * minute), (5 * minute) as u64)
        .unwrap();
    step(&mut engine, 8 * minute);
    engine
        .start(&review, at(8 * minute), (8 * minute) as u64)
        .unwrap();
    step(&mut engine, 10 * minute);
    engine.pause(at(10 * minute), (10 * minute) as u64).unwrap();
    engine
        .start(&qa, at(11 * minute), (11 * minute) as u64)
        .unwrap();
    step(&mut engine, 12 * minute);
    let session = engine
        .end_session(at(12 * minute), (12 * minute) as u64)
        .unwrap()
        .expect("session recorded");
    (session, [review, email, qa])
}

#[test]
fn aggregate_report_opens_from_history_with_all_timer_notes_and_copyable_summary() {
    let db = TempDb::new("aggregate-report-ui");
    let source;
    {
        let mut timer_engine = TimerEngine::boot(AppData::open(&db.0).unwrap(), at(0), 0).unwrap();
        let first = timer_engine.timers()[0].id.clone();
        let second = timer_engine.add_timer().unwrap();
        timer_engine
            .update_name_notes(
                &first,
                "Multiple dice types",
                "Implemented dice types.\nAdded unit tests.",
            )
            .unwrap();
        timer_engine
            .update_name_notes(&second, "Local multiplayer", "Local play flow.")
            .unwrap();
        timer_engine.start(&first, at(0), 0).unwrap();
        timer_engine
            .start(&second, at(1_200_000), 1_200_000)
            .unwrap();
        source = timer_engine
            .end_session(at(1_800_000), 1_800_000)
            .unwrap()
            .unwrap();
        let loaded = timer_engine
            .load_session(&source, LoadMode::AddToCurrent, at(3_600_000), 3_600_000)
            .unwrap();
        timer_engine
            .update_name_notes(
                &loaded[0],
                "Multiple dice types",
                "Added more dice variants.",
            )
            .unwrap();
        timer_engine
            .start(&loaded[0], at(3_600_000), 3_600_000)
            .unwrap();
        timer_engine
            .start(&loaded[1], at(4_200_000), 4_200_000)
            .unwrap();
        let third = timer_engine.add_timer().unwrap();
        timer_engine
            .update_name_notes(&third, "Documentation", "Updated the guide.")
            .unwrap();
        timer_engine
            .start(&third, at(4_500_000), 4_500_000)
            .unwrap();
        timer_engine.end_session(at(4_620_000), 4_620_000).unwrap();
    }
    let mut app = App::at_path(db.0.clone());
    click_text(&mut app, "History");
    click_id(&mut app, &format!("history-row-{source}"));
    let current = engine(&app).session().id.clone();
    let focus = app.focused_id.clone();
    let card = bounds_for(&app, "active-panel");
    click_id(&mut app, &format!("report-session-{source}"));
    assert_text(&app, "Session report");
    assert_text(&app, "00:47:00");
    assert_text(&app, "00:30:00");
    assert_text(&app, "00:15:00");
    assert_text(&app, "00:02:00");
    assert_text(&app, "Implemented dice types.\nAdded unit tests.");
    assert_text(&app, "Added more dice variants.");
    assert_text(&app, "Local play flow.");
    assert_text(&app, "Updated the guide.");
    assert_eq!(app.report.as_ref().unwrap().data.recordings.len(), 2);
    assert_eq!(app.focused_id, focus);
    assert_eq!(engine(&app).session().id, current);
    assert!(!app.report.as_ref().unwrap().show_recordings);
    for size in [SIZE, (1280.0, 800.0), (360.0, 480.0)] {
        for id in [
            "session-report",
            "report-copy",
            "report-refresh",
            "report-close",
            "report-scroll",
        ] {
            let bounds = bounds_at(&app, id, size);
            assert!(
                bounds.x >= 0.0 && bounds.x + bounds.width <= size.0,
                "{id}: {bounds:?}"
            );
            assert!(
                bounds.y >= 0.0 && bounds.y + bounds.height <= size.1,
                "{id}: {bounds:?}"
            );
        }
    }
    let copied = app.report_text().unwrap();
    assert!(copied.contains("Total tracked time: 00:47:00"));
    assert!(copied.contains("Multiple dice types — 00:30:00 (64%)"));
    assert!(copied.contains("    Implemented dice types.\n    Added unit tests."));
    assert_eq!(copied.matches("Local play flow.").count(), 1);
    click_id(&mut app, "report-copy");
    assert_eq!(app.report_text().unwrap(), copied);
    app.report.as_mut().unwrap().generated_at = at(4_620_000);
    snapshot(&app, SIZE, "lbx-timer-aggregate-report");
    snapshot(&app, (1280.0, 800.0), "lbx-timer-aggregate-report-wide");
    click_id(&mut app, "report-recordings-toggle");
    assert!(app.report.as_ref().unwrap().show_recordings);
    assert!(has_id(&app, &format!("report-recording-{source}")));
    click_id(&mut app, "report-close");
    assert!(app.report.is_none());
    assert_eq!(bounds_for(&app, "active-panel"), card);
    assert_eq!(app.focused_id, focus);
}

#[test]
fn live_aggregate_report_is_a_refreshable_snapshot_and_preserves_pending_verification() {
    let (_db, mut app) = hubstaff_app("aggregate-report-unverified");
    let id = timer_ids(&app)[0].clone();
    start_hubstaff_timer(&mut app, &id);
    let source = engine(&app).session().id.clone();
    let _ = app.update(Message::NotesChanged("Current timer notes".into()));
    report_hubstaff(&mut app, None, Err(ProviderError("CLI timeout".into())));
    let _ = app.update(Message::OpenReport(source.clone()));
    assert!(app.report.as_ref().unwrap().unverified);
    assert_text(&app, "Current timer notes");
    assert!(app
        .report_text()
        .unwrap()
        .contains("Includes Unverified time awaiting Hubstaff evidence."));
    assert!(engine(&app).running().is_some());
    let old_total = app.report.as_ref().unwrap().data.total_ms();
    app.monotonic_start -= Duration::from_secs(2);
    let _ = app.update(Message::Tick);
    assert_eq!(app.report.as_ref().unwrap().data.total_ms(), old_total);
    click_id(&mut app, "report-refresh");
    assert!(app.report.as_ref().unwrap().data.total_ms() >= old_total + 1900);
    assert!(engine(&app).running().is_some());
    assert_eq!(engine(&app).session().id, source);
    click_id(&mut app, "report-close");
    assert!(engine(&app).running().is_some());
}

#[test]
fn layout_discloses_panels_progressively_and_keeps_controls_stable() {
    let db = TempDb::new("layout");
    let mut app = App::at_path(db.0.clone());

    let idle_card = bounds_for(&app, "active-panel");
    assert!(
        idle_card.width <= 450.0,
        "timer card too wide: {idle_card:?}"
    );
    assert!(idle_card.height >= 180.0, "timer card slot: {idle_card:?}");
    let idle_primary = bounds_for(&app, "primary-action");
    let idle_end = bounds_for(&app, "end-session-action");
    assert_text(&app, "Details");
    assert_text(&app, "History");
    assert_text(&app, "Timer 1");
    assert_text(&app, "+ Add timer");
    assert!(!has_text(&app, "TIMER"));
    assert!(!has_text(&app, "Ready"));
    assert!(!has_id(&app, "task-name"), "details collapsed by default");
    assert!(!has_text(&app, "×"), "the only timer is removable");
    snapshot(&app, SIZE, "lbx-timer-idle");

    click_text(&mut app, "Details");
    click_text(&mut app, "History");
    assert!(app.details_expanded && app.history_expanded);
    assert!(!has_text(&app, "Task details"));
    assert!(!has_text(&app, "Saved tasks"));

    click_text(&mut app, "Start");
    assert!(app.details_expanded && app.history_expanded);
    let running_card = bounds_for(&app, "active-panel");
    assert_eq!(idle_card, running_card, "start changed timer card geometry");
    assert_eq!(idle_primary, bounds_for(&app, "primary-action"));
    assert_eq!(idle_end, bounds_for(&app, "end-session-action"));

    let _ = app.update(Message::Tick);
    assert_eq!(
        running_card,
        bounds_for(&app, "active-panel"),
        "tick moved card"
    );

    let _ = app.update(Message::NameChanged(
        "A very long task name that must not move the controls".into(),
    ));
    assert_eq!(running_card, bounds_for(&app, "active-panel"), "long name");
    let row_id = format!("timer-row-{}", timer_ids(&app)[0]);
    assert_eq!(bounds_for(&app, &row_id).height, TIMER_ROW_HEIGHT);

    click_text(&mut app, "Pause");
    assert_eq!(running_card, bounds_for(&app, "active-panel"), "pause");
    assert_eq!(idle_primary, bounds_for(&app, "primary-action"));
    click_text(&mut app, "Resume");
    assert_eq!(idle_primary, bounds_for(&app, "primary-action"));

    // Adding timers never moves the clock or its controls.
    click_text(&mut app, "+ Add timer");
    click_text(&mut app, "+ Add timer");
    assert_eq!(running_card, bounds_for(&app, "active-panel"), "add timer");
    assert_eq!(idle_primary, bounds_for(&app, "primary-action"));
    assert_eq!(idle_end, bounds_for(&app, "end-session-action"));

    app.notice = Some("Recovered the previous task; the uncertain gap was not counted.".into());
    assert_eq!(running_card, bounds_for(&app, "active-panel"), "notice");
    app.notice = None;
    app.error_message = Some("simulated persistence error".into());
    assert_eq!(running_card, bounds_for(&app, "active-panel"), "error");
    app.error_message = None;

    click_text(&mut app, "End session");
    assert!(app.details_expanded && app.history_expanded);
    assert_eq!(
        running_card,
        bounds_for(&app, "active-panel"),
        "end session"
    );
    assert_eq!(
        engine(&app).timers().len(),
        1,
        "fresh session has one timer"
    );
    assert_eq!(engine(&app).timers()[0].state, TimerState::Unstarted);
    assert_text(&app, "00:00:00");

    let key = app.history[0].id().to_owned();
    let history_row = format!("history-row-{key}");
    assert_eq!(bounds_for(&app, &history_row).height, HISTORY_ROW_HEIGHT);
    {
        let mut ui = iced_test::Simulator::with_size(Default::default(), SIZE, app.view());
        let title = ui
            .find(session_title(&app.history[0], false))
            .expect("session title rendered");
        assert!(
            title.bounds().height <= 24.0,
            "title wrapped: {:?}",
            title.bounds()
        );
    }

    let event_bounds = bounds_at(&app, "events-scroll", (480.0, 1000.0));
    assert!(
        event_bounds.width >= 360.0,
        "event list narrow: {event_bounds:?}"
    );
    // After End session, Details shows the new session's timer; select
    // the ended one from history to read its log.
    assert!(app.selected_log.is_empty(), "ended timer still in Details");
    let ended = app.history[0].tasks[0].id.clone();
    let _ = app.update(Message::SelectTask(ended));
    let first_entry = app.selected_log.first().expect("start entry").clone();
    assert!(
        first_entry.text.starts_with("Started “"),
        "{}",
        first_entry.text
    );
    {
        let mut event_ui =
            iced_test::Simulator::with_size(Default::default(), (480.0, 1000.0), app.view());
        assert!(
            event_ui.find(first_entry.text.as_str()).is_ok(),
            "log sentence should be rendered in full"
        );
        assert!(
            event_ui.find(log_time(&first_entry)).is_ok(),
            "event timestamp should remain fully readable"
        );
    }

    click_text(&mut app, "History");
    assert!(!app.history_expanded && app.details_expanded);
    click_text(&mut app, "Details");
    assert!(
        !has_id(&app, "task-name"),
        "details disclosure hides fields"
    );
}

#[test]
fn timer_list_fits_the_default_window_and_scrolls_after_four_rows() {
    let db = TempDb::new("list");
    let mut app = App::at_path(db.0.clone());
    let one_row = bounds_for(&app, "timers-scroll").height;
    assert_eq!(one_row, TIMER_ROW_HEIGHT);
    for _ in 0..3 {
        click_text(&mut app, "+ Add timer");
    }
    let four_rows = bounds_for(&app, "timers-scroll").height;
    assert_eq!(four_rows, 4.0 * TIMER_ROW_HEIGHT + 3.0 * TIMER_ROW_SPACING);
    // With four timers every control is on screen at the default size.
    for id in [
        "primary-action",
        "end-session-action",
        "add-timer",
        "history-disclosure",
    ] {
        let bounds = bounds_for(&app, id);
        assert!(
            bounds.y + bounds.height <= SIZE.1,
            "{id} is below the fold: {bounds:?}"
        );
    }
    snapshot(&app, SIZE, "lbx-timer-four-timers");

    for _ in 4..MAX_TIMERS {
        click_text(&mut app, "+ Add timer");
    }
    assert_eq!(engine(&app).timers().len(), MAX_TIMERS);
    assert_eq!(
        bounds_for(&app, "timers-scroll").height,
        four_rows,
        "list grew"
    );
    let _ = app.update(Message::AddTimer);
    assert_eq!(engine(&app).timers().len(), MAX_TIMERS, "limit ignored");
}

#[test]
fn any_row_starts_with_one_click_and_pauses_the_running_timer() {
    let db = TempDb::new("rows");
    let mut app = App::at_path(db.0.clone());
    click_text(&mut app, "+ Add timer");
    click_text(&mut app, "+ Add timer");
    let ids = timer_ids(&app);
    assert_eq!(ids.len(), 3);
    assert!(ids
        .iter()
        .all(|id| state_of(&app, id) == TimerState::Unstarted));
    assert!(
        engine(&app).history().unwrap().is_empty(),
        "adding stored rows"
    );

    click_id(&mut app, &format!("timer-toggle-{}", ids[0]));
    assert_eq!(state_of(&app, &ids[0]), TimerState::Running);
    assert_text(&app, "RUNNING");

    // Timer 2 starts straight from its row: no selecting it first.
    click_id(&mut app, &format!("timer-toggle-{}", ids[1]));
    assert_eq!(state_of(&app, &ids[0]), TimerState::Paused);
    assert_eq!(state_of(&app, &ids[1]), TimerState::Running);
    assert_eq!(app.focused_id.as_deref(), Some(ids[1].as_str()));
    assert_text(&app, "Timer 2");
    snapshot(&app, SIZE, "lbx-timer-switched");

    // Even while another timer is focused in the clock card, a row's
    // button acts on that row.
    click_id(&mut app, &format!("timer-row-{}", ids[2]));
    assert_eq!(app.focused_id.as_deref(), Some(ids[2].as_str()));
    assert_eq!(
        state_of(&app, &ids[1]),
        TimerState::Running,
        "focus started it"
    );
    click_id(&mut app, &format!("timer-toggle-{}", ids[0]));
    assert_eq!(state_of(&app, &ids[1]), TimerState::Paused);
    assert_eq!(state_of(&app, &ids[0]), TimerState::Running);
    assert_eq!(app.focused_id.as_deref(), Some(ids[0].as_str()));

    // The running row's button pauses the set; it reads Resume after.
    click_id(&mut app, &format!("timer-toggle-{}", ids[0]));
    assert!(engine(&app).running().is_none());
    assert_text(&app, "PAUSED");
    assert_text(&app, "Resume");
    click_id(&mut app, &format!("timer-toggle-{}", ids[1]));
    assert_eq!(state_of(&app, &ids[1]), TimerState::Running);

    let running: Vec<_> = engine(&app)
        .history()
        .unwrap()
        .into_iter()
        .filter(|t| t.state == TaskState::Running)
        .collect();
    assert_eq!(running.len(), 1);
}

#[test]
fn only_unstarted_extra_timers_can_be_removed() {
    let db = TempDb::new("remove");
    let mut app = App::at_path(db.0.clone());
    click_text(&mut app, "+ Add timer");
    let ids = timer_ids(&app);
    assert_text(&app, "×");
    click_id(&mut app, &format!("timer-row-{}", ids[1]));
    click_id(&mut app, &format!("timer-remove-{}", ids[1]));
    assert_eq!(timer_ids(&app), vec![ids[0].clone()]);
    assert_eq!(app.focused_id.as_deref(), Some(ids[0].as_str()));
    assert!(!has_text(&app, "×"));

    click_text(&mut app, "Start");
    click_text(&mut app, "+ Add timer");
    click_text(&mut app, "Pause");
    // A timer with recorded time cannot be removed mid-session.
    let _ = app.update(Message::RemoveTimer(ids[0].clone()));
    assert!(app.error_message.is_some());
    assert_eq!(engine(&app).timers().len(), 2);
}

#[test]
fn history_groups_linked_timers_with_totals_shares_and_timeline() {
    let db = TempDb::new("history");
    let (session_id, [review, email, qa]) = seed_linked_session(&db.0);
    let mut app = App::at_path(db.0.clone());
    click_text(&mut app, "History");
    assert_eq!(app.history.len(), 1);
    assert_eq!(app.history[0].total_ms(), 11 * 60_000);
    assert_text(&app, "00:11:00");
    assert_text(&app, "3 timers ▾");
    assert!(!has_id(&app, &format!("session-detail-{session_id}")));

    click_id(&mut app, &format!("history-row-{session_id}"));
    assert!(has_id(&app, &format!("session-detail-{session_id}")));
    assert!(has_id(&app, &format!("share-bar-{session_id}")));
    assert_text(&app, "3 timers ▴");
    for (id, share) in [(&review, "64%"), (&email, "27%"), (&qa, "9%")] {
        assert!(has_id(&app, &format!("history-task-{id}")));
        assert_text(&app, share);
    }
    assert_text(&app, "00:07:00");
    assert_text(&app, "Timer 3");

    let segments = &app.breakdowns[&session_id];
    assert_eq!(
        segments
            .iter()
            .map(|s| s.duration_ms / 60_000)
            .collect::<Vec<_>>(),
        vec![5, 3, 2, 1]
    );
    assert_text(&app, "Timeline");
    assert_text(&app, "Paused");
    {
        let mut ui =
            iced_test::Simulator::with_size(Default::default(), (480.0, 1200.0), app.view());
        assert!(ui.find("Email").is_ok());
    }

    // Seeded with injected clocks, so the render is already reproducible
    // (up to the machine's time zone) and keeps its idle gap.
    snapshot(&app, (480.0, 1200.0), "lbx-timer-history-breakdown");

    // Selecting a stored timer opens it in Details without touching the
    // new session's clock.
    click_text(&mut app, "Details");
    let focused = app.focused_id.clone();
    click_id(&mut app, &format!("history-task-{email}"));
    assert_eq!(app.selected_id.as_deref(), Some(email.as_str()));
    assert_eq!(app.focused_id, focused);
    assert_eq!(app.name, "Email");
    assert!(selected_events(&app)
        .iter()
        .any(|e| e.cause == Some(EventCause::Switch)));

    click_id(&mut app, &format!("history-row-{session_id}"));
    assert!(!has_id(&app, &format!("session-detail-{session_id}")));
}

#[test]
fn history_is_a_read_only_record() {
    let db = TempDb::new("record");
    let mut app = App::at_path(db.0.clone());
    let _ = app.update(Message::NameChanged("Review".into()));
    let _ = app.update(Message::NotesChanged("queue 7".into()));
    click_text(&mut app, "Start");
    let saved_id = app.selected_id.clone().expect("started timer is selected");
    click_text(&mut app, "End session");

    click_text(&mut app, "History");
    click_text(&mut app, "Details");
    let key = app.history[0].id().to_owned();
    click_id(&mut app, &format!("history-row-{key}"));
    stabilize_visual_timestamps(&mut app);
    // No way to delete or rename from History.
    assert!(!has_text(&app, "Delete"));
    assert!(!has_text(&app, "Current"));
    assert_eq!(
        bounds_for(&app, &format!("history-task-{saved_id}")).height,
        HISTORY_TASK_ROW_HEIGHT
    );

    // Selecting a past timer shows it read-only.
    click_id(&mut app, &format!("history-task-{saved_id}"));
    assert_eq!(app.selected_id.as_deref(), Some(saved_id.as_str()));
    assert!(has_id(&app, "details-record"));
    assert!(
        !has_id(&app, "task-name"),
        "past timer shows an editable name"
    );
    assert_text(&app, "Review");
    assert_text(&app, "queue 7");
    assert!(!app.selected_log.is_empty());
    let _ = app.update(Message::NameChanged("Rewritten".into()));
    let _ = app.update(Message::NotesChanged("Rewritten".into()));
    let stored = engine(&app).task(&saved_id).unwrap().unwrap();
    assert_eq!(
        (stored.name.as_str(), stored.notes.as_str()),
        ("Review", "queue 7")
    );
    stabilize_visual_timestamps(&mut app);
    snapshot(&app, (480.0, 1400.0), "lbx-timer-history");

    // The current session's timers stay editable.
    let current = timer_ids(&app)[0].clone();
    click_id(&mut app, &format!("timer-row-{current}"));
    assert!(has_id(&app, "task-name"));
    assert!(!has_id(&app, "details-record"));
}

#[test]
fn start_pause_resume_end_session_and_reopen() {
    let db = TempDb::new("flow");
    let mut app = App::at_path(db.0.clone());
    assert!(engine(&app).running().is_none());
    assert!(app.history.is_empty());
    assert_text(&app, "00:00:00");
    assert_text(&app, "Start");

    click_text(&mut app, "Start");
    let first_id = app.selected_id.clone().expect("Start selects the timer");
    assert_eq!(state_of(&app, &first_id), TimerState::Running);
    assert_text(&app, "RUNNING");
    assert_text(&app, "Timer 1");

    click_text(&mut app, "Details");
    type_name(&mut app, "Batch review");
    assert_eq!(app.name, "Batch review");
    assert_eq!(
        engine(&app).task(&first_id).unwrap().unwrap().name,
        "Batch review"
    );

    click_text(&mut app, "Pause");
    assert_eq!(state_of(&app, &first_id), TimerState::Paused);
    assert_text(&app, "PAUSED");
    click_text(&mut app, "Resume");
    assert_eq!(state_of(&app, &first_id), TimerState::Running);

    click_text(&mut app, "End session");
    assert!(engine(&app).running().is_none());
    assert_eq!(app.history.len(), 1);
    assert_eq!(app.history[0].tasks[0].state, TaskState::Completed);
    assert_text(&app, "00:00:00");
    assert_text(&app, "Start");
    let session = engine(&app).session().id.clone();

    drop(app);
    let mut reopened = App::at_path(db.0.clone());
    assert_ne!(engine(&reopened).session().id, session);
    assert_eq!(engine(&reopened).timers().len(), 1);
    assert_eq!(reopened.history.len(), 1);
    assert_eq!(reopened.history[0].tasks[0].id, first_id);
    assert_text(&reopened, "00:00:00");

    click_text(&mut reopened, "Start");
    let second_id = reopened.selected_id.clone().expect("fresh timer");
    assert_ne!(second_id, first_id);
    assert_eq!(reopened.history.len(), 2);
    assert_text(&reopened, "RUNNING");
}

#[test]
fn details_autosave_for_unstarted_running_and_ended_timers() {
    let db = TempDb::new("details");
    let mut app = App::at_path(db.0.clone());
    let id = app
        .selected_id
        .clone()
        .expect("first timer selected at launch");
    // Naming an unstarted timer stores nothing until it starts.
    let _ = app.update(Message::NameChanged("Batch review".into()));
    assert!(engine(&app).history().unwrap().is_empty());
    assert_text(&app, "Batch review");
    let _ = app.update(Message::Start(id.clone()));
    let _ = app.update(Message::NotesChanged("Remember this".into()));
    let _ = app.update(Message::Pause);
    assert_eq!(
        selected_events(&app).last().unwrap().kind,
        lbx_timer::EventKind::Pause
    );
    let _ = app.update(Message::Start(id.clone()));
    app.notice = Some("stale notice".into());
    let _ = app.update(Message::EndSession);
    assert!(engine(&app).running().is_none());
    // Ending the session clears Details: it shows the new session's
    // fresh timer, with nothing left over from the ended one.
    let next = timer_ids(&app)[0].clone();
    assert_eq!(app.selected_id.as_deref(), Some(next.as_str()));
    assert_eq!(app.focused_id.as_deref(), Some(next.as_str()));
    assert!(app.name.is_empty() && app.notes.is_empty());
    assert!(selected_events(&app).is_empty() && app.selected_log.is_empty());
    assert!(app.notice.is_none() && app.error_message.is_none());

    // The ended timer is part of the record: viewable, no longer editable.
    let _ = app.update(Message::SelectTask(id.clone()));
    assert_eq!(app.name, "Batch review");
    assert_eq!(app.notes, "Remember this");
    assert_eq!(
        selected_events(&app).last().unwrap().cause,
        Some(EventCause::EndSession)
    );
    let _ = app.update(Message::NameChanged("Renamed after end".into()));
    assert_eq!(app.name, "Batch review");
    drop(app);
    let data = AppData::open(&db.0).unwrap();
    assert_eq!(data.task(&id).unwrap().unwrap().name, "Batch review");
}

/// Click the Name field and type, all in one simulator session so the
/// field keeps its focus and selection between the steps.
fn click_name_and_type(app: &mut App, clicks: usize, typed: &str) -> Option<String> {
    let messages = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), TALL, app.view());
        for click in 0..clicks {
            if click > 0 {
                // Slower than iced's 300 ms double-click window, like a
                // person clicking again to place the cursor.
                std::thread::sleep(Duration::from_millis(350));
            }
            ui.click(iced_test::selector::id("task-name"))
                .expect("name field present");
        }
        let _ = ui.typewrite(typed);
        ui.into_messages().collect::<Vec<_>>()
    };
    let last = messages.iter().rev().find_map(|message| match message {
        Message::NameChanged(value) => Some(value.clone()),
        _ => None,
    });
    deliver(app, messages);
    last
}

#[test]
fn name_field_shows_the_default_name_and_selects_a_custom_name_on_focus() {
    let db = TempDb::new("name");
    let mut app = App::at_path(db.0.clone());
    click_text(&mut app, "Details");
    // Unnamed: the field is empty and shows the same "Timer 1" as the
    // clock card; typing starts from a clear field.
    assert!(app.name.is_empty());
    assert_eq!(app.default_name(), "Timer 1");
    assert_eq!(
        click_name_and_type(&mut app, 1, "Review").as_deref(),
        Some("Review")
    );
    assert_text(&app, "Review");

    // Named: the click that focuses the field selects the whole name, so
    // typing replaces it.
    assert_eq!(
        click_name_and_type(&mut app, 1, "QA").as_deref(),
        Some("QA")
    );
    assert_eq!(app.name, "QA");

    // A second click places the cursor instead, so the name can be
    // edited in place (here: at the end of the short name).
    assert_eq!(
        click_name_and_type(&mut app, 2, " pass").as_deref(),
        Some("QA pass")
    );
    assert_eq!(app.name, "QA pass");

    // Clearing the name brings the default back everywhere.
    let _ = app.update(Message::NameChanged(String::new()));
    assert_eq!(app.default_name(), "Timer 1");
    assert_text(&app, "Timer 1");
    click_text(&mut app, "+ Add timer");
    let second = timer_ids(&app)[1].clone();
    click_id(&mut app, &format!("timer-row-{second}"));
    assert_eq!(app.default_name(), "Timer 2");
}

#[test]
fn wide_windows_use_two_centered_panes() {
    let db = TempDb::new("wide");
    let (_, _) = seed_linked_session(&db.0);
    let mut app = App::at_path(db.0.clone());
    let wide = (1280.0, 800.0);
    // Details and History are open beside the timers; no disclosures.
    assert!(has_id_at(&app, "wide-layout", wide));
    assert!(has_id_at(&app, "details-panel", wide));
    assert!(has_id_at(&app, "history-panel", wide));
    assert!(!has_id_at(&app, "details-disclosure", wide));
    let card = bounds_at(&app, "active-panel", wide);
    let details = bounds_at(&app, "details-panel", wide);
    let history = bounds_at(&app, "history-panel", wide);
    assert!(
        details.x >= card.x + card.width,
        "details not beside the timer"
    );
    assert!(
        details.width >= 440.0,
        "details pane too narrow: {details:?}"
    );
    assert!(history.y > details.y);
    for id in ["active-panel", "details-panel", "history-panel"] {
        let bounds = bounds_at(&app, id, wide);
        assert!(
            bounds.x + bounds.width <= wide.0,
            "{id} overflows: {bounds:?}"
        );
        assert!(
            bounds.y + bounds.height <= wide.1,
            "{id} is cut off: {bounds:?}"
        );
    }
    // Snapshot before anything runs on the real clock, so it is stable.
    snapshot(&app, wide, "lbx-timer-wide");

    // History stays live in the wide layout although its narrow-layout
    // disclosure is collapsed.
    assert!(!app.history_expanded);
    let id = timer_ids(&app)[0].clone();
    let _ = app.update(Message::Start(id.clone()));
    std::thread::sleep(Duration::from_millis(1_100));
    let _ = app.update(Message::Tick);
    let live = engine(&app).timer(&id).unwrap().elapsed_ms;
    let current = app
        .history
        .iter()
        .find(|summary| summary.id() == engine(&app).session().id)
        .expect("current session in history");
    assert_eq!(current.total_ms(), live, "history lags the running timer");
    let _ = app.update(Message::Pause);

    // Very wide windows center the content instead of stretching it.
    let huge = (2400.0, 900.0);
    let card = bounds_at(&app, "active-panel", huge);
    let details = bounds_at(&app, "details-panel", huge);
    let left_margin = card.x;
    let right_margin = huge.0 - (details.x + details.width);
    assert!((left_margin - right_margin).abs() <= 2.0, "not centered");
    assert!(left_margin > 500.0, "content stretched: {card:?}");

    // The timers keep working in the wide layout.
    let id = timer_ids(&app)[0].clone();
    let messages = {
        let mut ui = iced_test::Simulator::with_size(Default::default(), wide, app.view());
        ui.click(iced_test::selector::id(format!("timer-toggle-{id}")))
            .expect("toggle in wide layout");
        ui.into_messages().collect::<Vec<_>>()
    };
    deliver(&mut app, messages);
    assert_eq!(state_of(&app, &id), TimerState::Running);
}

#[test]
fn narrow_windows_fill_the_width_up_to_the_column_limit() {
    let db = TempDb::new("narrow");
    let app = App::at_path(db.0.clone());
    let default_card = bounds_at(&app, "active-panel", SIZE);
    let medium = bounds_at(&app, "active-panel", (760.0, 720.0));
    assert!(medium.width > default_card.width, "column did not grow");
    assert_eq!(medium.width, NARROW_MAX_WIDTH);
    assert!(
        (medium.x - (760.0 - medium.width) / 2.0).abs() <= 1.0,
        "not centered"
    );
    assert!(has_id_at(&app, "details-disclosure", (760.0, 720.0)));
}

#[test]
fn row_buttons_are_square_icons_with_hover_labels() {
    let db = TempDb::new("icons");
    let mut app = App::at_path(db.0.clone());
    click_text(&mut app, "+ Add timer");
    let ids = timer_ids(&app);
    let _ = app.update(Message::Start(ids[0].clone()));
    let _ = app.update(Message::Start(ids[1].clone()));
    let toggle = bounds_for(&app, &format!("timer-toggle-{}", ids[0]));
    assert_eq!(toggle.width, TIMER_ROW_HEIGHT, "toggle is not square");
    assert_eq!(toggle.height, TIMER_ROW_HEIGHT);

    // The clock card shows Timer 2 (Pause), so no "Resume" text is on
    // screen until Timer 1's icon button is hovered. The simulator does
    // not search tooltip overlays, so the hover label is checked in the
    // review snapshot instead.
    let mut ui = iced_test::Simulator::with_size(Default::default(), SIZE, app.view());
    assert!(ui.find("Resume").is_err(), "label visible without hovering");
    ui.point_at(toggle.center());
    let hover = iced::Event::Mouse(iced::mouse::Event::CursorMoved {
        position: toggle.center(),
    });
    let _ = ui.simulate([hover.clone()]);
    std::thread::sleep(TOOLTIP_DELAY + Duration::from_millis(100));
    let _ = ui.simulate([hover]);
    assert!(ui
        .snapshot(&Theme::Dark)
        .unwrap()
        .matches_image("target/ui-snapshots/lbx-timer-row-hover.png")
        .unwrap());
}

#[test]
fn resumed_timer_set_shows_combined_total_without_moving_rows_or_double_counting_history() {
    let db = TempDb::new("combined-session-total");
    let (source, _) = seed_linked_session(&db.0);
    let mut app = App::at_path(db.0.clone());
    let fresh_header = bounds_for(&app, "session-total");
    let fresh_list_y = bounds_for(&app, "timers-scroll").y;
    assert!(!has_text(&app, "Across sessions"));
    click_text(&mut app, "History");
    click_id(&mut app, &format!("history-row-{source}"));
    click_id(&mut app, &format!("resume-session-{source}"));
    click_text(&mut app, "History");
    assert_text(&app, "This session");
    assert_text(&app, "Across sessions");
    assert_text(&app, "00:11:00");
    assert_eq!(bounds_for(&app, "session-total"), fresh_header);
    assert_eq!(bounds_for(&app, "timers-scroll").y, fresh_list_y);
    click_text(&mut app, "+ Add timer");
    for id in [
        "session-total-across-sessions",
        "add-timer",
        "history-disclosure",
    ] {
        let bounds = bounds_for(&app, id);
        assert!(bounds.x >= 0.0 && bounds.x + bounds.width <= SIZE.0);
        assert!(
            bounds.y >= 0.0 && bounds.y + bounds.height <= SIZE.1,
            "{id}: {bounds:?}"
        );
    }
    for id in timer_ids(&app) {
        let bounds = bounds_for(&app, &format!("timer-row-{id}"));
        assert!(bounds.y + bounds.height <= SIZE.1);
    }
    snapshot(&app, SIZE, "lbx-timer-combined-session-total");

    // Inject a minute on the continued timer and thirty seconds on the new
    // timer. Both contribute to the combined total; only new time is current.
    let ids = timer_ids(&app);
    let (now, mono) = app.now_and_mono();
    let timer_engine = app.engine.as_mut().unwrap();
    timer_engine.start(&ids[0], now, mono).unwrap();
    // Touch the other continuations so all four timers belong to the saved
    // session, including those with no additional time in this session.
    for id in &ids[1..3] {
        timer_engine
            .start(id, now + chrono::Duration::minutes(1), mono + 60_000)
            .unwrap();
    }
    timer_engine
        .start(&ids[3], now + chrono::Duration::minutes(1), mono + 60_000)
        .unwrap();
    timer_engine
        .pause(now + chrono::Duration::seconds(90), mono + 90_000)
        .unwrap();
    app.monotonic_start -= Duration::from_secs(90);
    app.refresh_history();
    assert_text(&app, "00:01:30");
    assert_text(&app, "00:12:30");
    assert_eq!(bounds_for(&app, "session-total"), fresh_header);

    // Continue the result again: its earlier eleven minutes must count once.
    click_text(&mut app, "End session");
    assert!(!has_text(&app, "Across sessions"));
    assert_eq!(bounds_for(&app, "timers-scroll").y, fresh_list_y);
    let newer = app
        .history
        .iter()
        .find(|summary| summary.id() != source)
        .unwrap()
        .id()
        .to_owned();
    click_text(&mut app, "History");
    click_id(&mut app, &format!("history-row-{newer}"));
    click_id(&mut app, &format!("resume-session-{newer}"));
    assert_text(&app, "00:12:30");
    assert_eq!(engine(&app).session().total_ms(), 0);
}

#[test]
fn resuming_a_session_continues_its_timers_and_links_the_sessions() {
    let db = TempDb::new("load");
    let (source, [review, ..]) = seed_linked_session(&db.0);
    let mut app = App::at_path(db.0.clone());
    click_text(&mut app, "History");
    click_id(&mut app, &format!("history-row-{source}"));
    assert!(has_id(&app, &format!("resume-session-{source}")));
    click_id(&mut app, &format!("resume-session-{source}"));
    assert!(
        app.pending_resume.is_none(),
        "asked although nothing was recorded"
    );

    // The placeholder timer was replaced by the three loaded timers.
    let labels: Vec<String> = engine(&app).timers().iter().map(|t| t.label()).collect();
    assert_eq!(labels, ["Review", "Email", "Timer 3"]);
    assert!(engine(&app).timers().iter().all(|t| t.is_continued()));
    let first = timer_ids(&app)[0].clone();
    assert_eq!(app.focused_id.as_deref(), Some(first.as_str()));
    assert_eq!(app.name, "Review");

    // The running total is visible without replacing this session's time.
    assert_text(&app, "IDLE · 00:07:00 in total");
    assert_text(&app, "00:07:00 total");
    assert_text(&app, "00:00:00");
    click_text(&mut app, "Details");
    assert_eq!(
        app.selected_totals().as_deref(),
        Some("This session 00:00:00 · 00:07:00 in total across 1 session")
    );
    snapshot(&app, (480.0, 1200.0), "lbx-timer-resumed");

    click_id(&mut app, &format!("timer-toggle-{first}"));
    assert_eq!(state_of(&app, &first), TimerState::Running);
    assert!(app.selected_log.first().is_some_and(|entry| entry
        .text
        .starts_with("Started “Review” — continues “Review”")));
    click_text(&mut app, "End session");

    // History links the two sessions in both directions.
    let newer = app
        .history
        .iter()
        .find(|s| s.id() != source)
        .expect("new session stored")
        .clone();
    assert_eq!(newer.continues_sessions, vec![source.clone()]);
    let tag = app.link_tag(&newer).expect("continuation tag");
    assert!(tag.starts_with("continues "), "{tag}");
    assert_text(&app, &format!("{tag} · 1 timer ▾"));
    let older = app.history.iter().find(|s| s.id() == source).unwrap();
    assert_eq!(app.link_tag(older).as_deref(), Some("continued later"));

    // Following the link from the newer session expands the older one.
    let _ = app.update(Message::ToggleSession(source.clone()));
    assert!(!app.expanded_sessions.contains(&source));
    click_id(&mut app, &format!("history-row-{}", newer.id()));
    let continues = app
        .history
        .iter()
        .find(|s| s.id() == source)
        .map(|s| format!("Continues {}", date_time(s.session.started_at)))
        .unwrap();
    click_text(&mut app, &continues);
    assert!(app.expanded_sessions.contains(&source));

    // The original's history row shows the running total of its work.
    let review_series = engine(&app).task(&review).unwrap().unwrap().series_id;
    assert!(app.series_totals[&review_series].sessions == 2);
}

#[test]
fn resuming_while_the_session_has_time_asks_how() {
    let db = TempDb::new("load-ask");
    let (source, _) = seed_linked_session(&db.0);
    let mut app = App::at_path(db.0.clone());
    click_text(&mut app, "Start");
    let current = engine(&app).session().id.clone();
    click_text(&mut app, "History");
    click_id(&mut app, &format!("history-row-{source}"));

    click_id(&mut app, &format!("resume-session-{source}"));
    assert_eq!(app.pending_resume.as_deref(), Some(source.as_str()));
    assert_text(&app, "Resume this session?");
    snapshot(&app, (480.0, 1000.0), "lbx-timer-resume-confirmation");
    click_id(&mut app, "resume-cancel");
    assert!(app.pending_resume.is_none());
    assert_eq!(engine(&app).timers().len(), 1, "cancel changed the timers");

    click_id(&mut app, &format!("resume-session-{source}"));
    click_id(&mut app, "resume-add");
    assert_eq!(engine(&app).session().id, current);
    assert_eq!(engine(&app).timers().len(), 4);
    assert!(
        engine(&app).running().is_some(),
        "adding paused the running timer"
    );

    click_id(&mut app, &format!("resume-session-{source}"));
    click_id(&mut app, "resume-add");
    assert!(
        app.error_message.is_some(),
        "duplicate load was not reported"
    );
    assert_eq!(engine(&app).timers().len(), 4);

    click_id(&mut app, &format!("resume-session-{source}"));
    click_id(&mut app, "resume-end-first");
    assert_ne!(engine(&app).session().id, current);
    assert_eq!(engine(&app).timers().len(), 3);
    assert!(engine(&app).running().is_none());
    assert!(app
        .history
        .iter()
        .any(|s| s.id() == current && s.session.ended_at.is_some()));
    assert!(app.error_message.is_none());
}
