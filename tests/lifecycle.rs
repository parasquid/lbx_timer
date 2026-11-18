use chrono::{TimeZone, Utc};
use lbx_timer::storage::{NewTimer, SwitchTarget};
use lbx_timer::{AppData, AppError, EventKind, SessionEnd, TaskState, TimerEngine, TimerState};
use std::path::PathBuf;
use uuid::Uuid;

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("lbx-lifecycle-{}.db", Uuid::new_v4())))
    }
    fn open(&self) -> AppData {
        AppData::open(&self.0).unwrap()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        for suffix in ["", ".lock", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}
fn at(ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).unwrap()
}

#[test]
fn close_reopen_retains_identity_details_and_excludes_closed_gap() {
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
    let id = engine.timers()[0].id.clone();
    engine.start(&id, at(0), 0).unwrap();
    engine
        .update_name_notes(&id, "Review batch", "draft notes")
        .unwrap();
    engine.pause_for_close(at(5000), 5000).unwrap();
    drop(engine);
    let mut engine = TimerEngine::boot(db.open(), at(60000), 0).unwrap();
    assert!(engine.running().is_none());
    let previous = engine.task(&id).unwrap().unwrap();
    assert_eq!(previous.name, "Review batch");
    assert_eq!(previous.state, TaskState::Paused);
    assert_eq!(previous.elapsed_ms, 5000);
    let events = engine.events(&id).unwrap();
    assert_eq!(
        events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        [EventKind::Start, EventKind::ClosePause]
    );
    engine
        .update_name_notes(&id, "Renamed later", "after stop")
        .unwrap();
    let new_id = engine.timers()[0].id.clone();
    assert_ne!(new_id, id);
    engine.start(&new_id, at(63000), 3000).unwrap();
    assert_eq!(engine.running().unwrap().elapsed_ms, 0);
    engine.end_session(at(65000), 5000).unwrap();
    let task = engine.task(&new_id).unwrap().unwrap();
    assert_eq!(task.elapsed_ms, 2000);
    assert_eq!(task.state, TaskState::Completed);
    assert!(engine.running().is_none());
    assert_eq!(engine.task(&id).unwrap().unwrap().notes, "after stop");
}

#[test]
fn hubstaff_pause_reason_survives_reopen_without_a_schema_change() {
    let db = Database::new();
    let task_id;
    {
        let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
        task_id = engine.timers()[0].id.clone();
        engine.start(&task_id, at(0), 0).unwrap();
        engine
            .pause_with_reason(
                at(4_000),
                4_000,
                Some("Hubstaff reported tracking stopped; paused when observed."),
            )
            .unwrap();
        assert!(engine.events(&task_id).unwrap()[1]
            .details
            .as_deref()
            .unwrap()
            .contains("Hubstaff reported"));
    }
    let engine = TimerEngine::boot(db.open(), at(5_000), 0).unwrap();
    let log = engine.event_log(&task_id).unwrap();
    assert_eq!(log.len(), 2);
    assert!(log[1].text.contains("Hubstaff reported tracking stopped"));
}

#[test]
fn event_triggers_survive_reopen_alongside_legacy_events() {
    let db = Database::new();
    let first;
    let second;
    {
        let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
        first = engine.timers()[0].id.clone();
        second = engine.add_timer().unwrap();
        engine.start(&first, at(0), 0).unwrap();
        engine.start(&second, at(1000), 1000).unwrap();
        engine.pause(at(2000), 2000).unwrap();
        engine
            .start_with_reason(
                &second,
                at(3000),
                3000,
                "Triggered by Hubstaff: tracking confirmed after an automatic pause.",
            )
            .unwrap();
        engine.end_session(at(4000), 4000).unwrap();
    }
    // Represent a pre-existing event whose writer did not record provenance.
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute(
        "UPDATE events SET details = NULL WHERE task_id = ?1 AND kind = 'start'",
        [&first],
    )
    .unwrap();
    let before: (i64, i64) = conn
        .query_row("SELECT count(*), sum(elapsed_ms) FROM tasks", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    let data = db.open();
    assert_eq!(data.events(&first).unwrap()[0].details, None);
    drop(data);
    let engine = TimerEngine::boot(db.open(), at(5000), 0).unwrap();
    let first_log = engine.event_log(&first).unwrap();
    assert!(first_log[0]
        .text
        .contains("trigger not recorded (older event)"));
    assert!(first_log[1]
        .text
        .contains("switched to “Timer 2”; Triggered manually in LBX."));
    let log = engine.event_log(&second).unwrap();
    assert!(log[0]
        .text
        .contains("switched from “Timer 1”; Triggered manually in LBX."));
    assert!(log[1].text.contains("Triggered manually in LBX."));
    assert!(log[2]
        .text
        .contains("Triggered by Hubstaff: tracking confirmed"));
    assert!(log[3].text.contains("session was ended manually in LBX"));
    let after: (i64, i64) = conn
        .query_row("SELECT count(*), sum(elapsed_ms) FROM tasks", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(before, after);
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, lbx_timer::storage::SCHEMA_VERSION);
}

#[test]
fn schema_five_migrates_without_changing_existing_records_and_reopens() {
    let db = Database::new();
    let task_id;
    {
        let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
        task_id = engine.timers()[0].id.clone();
        engine.start(&task_id, at(0), 0).unwrap();
        engine
            .update_name_notes(&task_id, "Existing work", "keep these notes")
            .unwrap();
        engine.end_session(at(5000), 5000).unwrap();
    }
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch("DROP TABLE time_verifications; PRAGMA user_version = 5;")
        .unwrap();
    let original = std::fs::read(&db.0).unwrap();
    {
        let data = db.open();
        let task = data.task(&task_id).unwrap().unwrap();
        assert_eq!(
            (task.name.as_str(), task.notes.as_str(), task.elapsed_ms),
            ("Existing work", "keep these notes", 5000)
        );
        assert_eq!(data.events(&task_id).unwrap().len(), 2);
        assert!(data.time_verifications().unwrap().is_empty());
    }
    assert!(!original.is_empty());
    let data = db.open();
    assert_eq!(data.task(&task_id).unwrap().unwrap().elapsed_ms, 5000);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        lbx_timer::storage::SCHEMA_VERSION
    );
    assert!(conn.prepare("SELECT * FROM time_verifications").is_ok());
}

#[test]
fn automatic_reconciliation_is_exactly_once_and_preserves_raw_time_and_resume_totals() {
    use lbx_timer::verification::{VerificationResult, VerificationState};
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
    let id = engine.timers()[0].id.clone();
    engine.start(&id, at(0), 0).unwrap();
    let session = engine.session().id.clone();
    let checkpoint = engine.verified_checkpoint(at(1000), 1000).unwrap();
    engine
        .mark_time_unverified(checkpoint.as_ref(), "CLI timeout", at(2000), 2000)
        .unwrap();
    engine
        .mark_time_unverified(checkpoint.as_ref(), "another timeout", at(3000), 3000)
        .unwrap();
    assert_eq!(engine.time_verifications().unwrap().len(), 1);
    engine.verified_checkpoint(at(9000), 9000).unwrap();
    let interval = engine.time_verifications().unwrap()[0].clone();
    let result = VerificationResult {
        id: interval.id,
        ended_at: at(9000),
        state: VerificationState::Adjusted,
        evidence: "Automatically excluded 2.500 seconds using Hubstaff audit.".into(),
        excluded_ranges: vec![(at(3000), at(5500))],
    };
    engine
        .save_verification_results(std::slice::from_ref(&result))
        .unwrap();
    engine.save_verification_results(&[result]).unwrap();
    assert_eq!(engine.running().unwrap().elapsed_ms, 6500);
    assert_eq!(engine.task(&id).unwrap().unwrap().elapsed_ms, 6500);
    engine.tick(at(10000), 10000).unwrap();
    assert_eq!(engine.running().unwrap().elapsed_ms, 7500);
    engine.pause(at(11000), 11000).unwrap();
    engine.end_session(at(12000), 12000).unwrap();
    let segments = engine.session_segments(&session).unwrap();
    assert_eq!(
        segments
            .iter()
            .map(|s| (s.started_at.timestamp_millis(), s.duration_ms))
            .collect::<Vec<_>>(),
        [(0, 3000), (5500, 5500)]
    );
    assert_eq!(lbx_timer::breakdown::totals_by_task(&segments)[&id], 8500);
    assert_eq!(
        engine.events(&id).unwrap()[1].elapsed_ms,
        11000,
        "original local record was overwritten"
    );
    let log = engine.event_log(&id).unwrap();
    assert!(log.iter().any(|line| line.text.contains("Adjusted time")));
    assert_eq!(log.last().unwrap().elapsed_ms, 8500);
    drop(engine);
    let mut engine = TimerEngine::boot(db.open(), at(13000), 0).unwrap();
    assert_eq!(engine.session_history().unwrap()[0].total_ms(), 8500);
    assert_eq!(
        engine
            .session_report(&session, at(13000))
            .unwrap()
            .unwrap()
            .total_ms(),
        8500
    );
    let ids = engine
        .load_session(&session, lbx_timer::LoadMode::AddToCurrent, at(13000), 0)
        .unwrap();
    assert_eq!(engine.timer(&ids[0]).unwrap().total_ms(), 8500);
    assert!(engine.running().is_none());
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    assert_eq!(
        conn.query_row("SELECT elapsed_ms FROM tasks WHERE id = ?1", [&id], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        11000
    );
}

#[test]
fn reconciled_time_stays_correct_through_switches_and_later_resumes() {
    use lbx_timer::verification::{VerificationResult, VerificationState};
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
    let first = engine.timers()[0].id.clone();
    let second = engine.add_timer().unwrap();
    engine.start(&first, at(0), 0).unwrap();
    let session = engine.session().id.clone();
    let checkpoint = engine.verified_checkpoint(at(1000), 1000).unwrap();
    engine
        .mark_time_unverified(checkpoint.as_ref(), "timeout", at(2000), 2000)
        .unwrap();
    engine.verified_checkpoint(at(9000), 9000).unwrap();
    let interval = engine.time_verifications().unwrap()[0].clone();
    engine
        .save_verification_results(&[VerificationResult {
            id: interval.id,
            ended_at: at(9000),
            state: VerificationState::Adjusted,
            evidence: "Automatically adjusted".into(),
            excluded_ranges: vec![(at(3000), at(5500))],
        }])
        .unwrap();
    engine.start(&second, at(10000), 10000).unwrap();
    assert_eq!(engine.timer(&first).unwrap().elapsed_ms, 7500);
    engine.start(&first, at(12000), 12000).unwrap();
    engine.pause(at(13000), 13000).unwrap();
    assert_eq!(engine.timer(&first).unwrap().elapsed_ms, 8500);
    assert_eq!(engine.timer(&second).unwrap().elapsed_ms, 2000);
    assert_eq!(engine.session().total_ms(), 10500);
    assert_eq!(
        engine
            .session_segments(&session)
            .unwrap()
            .iter()
            .map(|segment| segment.duration_ms)
            .sum::<i64>(),
        10500
    );
    assert_eq!(engine.task(&first).unwrap().unwrap().elapsed_ms, 8500);
}

#[test]
fn invalid_reconciliation_rolls_back_without_losing_local_time() {
    use lbx_timer::verification::{VerificationResult, VerificationState};
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
    let id = engine.timers()[0].id.clone();
    engine.start(&id, at(0), 0).unwrap();
    let checkpoint = engine.verified_checkpoint(at(1000), 1000).unwrap();
    engine
        .mark_time_unverified(checkpoint.as_ref(), "timeout", at(2000), 2000)
        .unwrap();
    engine.verified_checkpoint(at(9000), 9000).unwrap();
    let interval = engine.time_verifications().unwrap()[0].clone();
    let invalid = VerificationResult {
        id: interval.id,
        ended_at: at(9000),
        state: VerificationState::Adjusted,
        evidence: "invalid boundaries".into(),
        excluded_ranges: vec![(at(3000), at(5000)), (at(4000), at(6000))],
    };
    assert!(engine.save_verification_results(&[invalid]).is_err());
    assert_eq!(engine.running().unwrap().elapsed_ms, 9000);
    assert_eq!(engine.task(&id).unwrap().unwrap().elapsed_ms, 9000);
    assert_eq!(
        engine.time_verifications().unwrap()[0].state,
        VerificationState::Unverified
    );
}

#[test]
fn unverified_intervals_survive_end_close_and_crash_boundaries() {
    let db = Database::new();
    {
        let mut data = db.open();
        data.switch("s", None, SwitchTarget::Start(new_timer("a", 1)), at(0))
            .unwrap();
        data.checkpoint("a", at(1000), 1000).unwrap();
        data.begin_time_verification("a", at(1000), 1000, "CLI timeout")
            .unwrap();
        data.checkpoint("a", at(5000), 5000).unwrap();
        // Dropping AppData simulates the process disappearing without a close event.
    }
    let engine = TimerEngine::boot(db.open(), at(60000), 0).unwrap();
    let intervals = engine.time_verifications().unwrap();
    assert_eq!(intervals[0].ended_at, Some(at(5000)));
    assert_eq!(intervals[0].elapsed_end_ms, Some(5000));
    assert!(engine
        .event_log("a")
        .unwrap()
        .iter()
        .any(|line| line.text.contains("Unverified time")));
}

#[test]
fn aggregate_reports_retain_all_notes_include_unstarted_work_and_do_not_rewrite_records() {
    let db = Database::new();
    let source;
    {
        let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
        let first = engine.timers()[0].id.clone();
        let second = engine.add_timer().unwrap();
        engine
            .update_name_notes(&first, "Dice", "Initial notes\nSecond line")
            .unwrap();
        engine
            .update_name_notes(&second, "Multiplayer", "Play flow")
            .unwrap();
        engine.start(&first, at(0), 0).unwrap();
        engine.start(&second, at(5000), 5000).unwrap();
        source = engine.end_session(at(10000), 10000).unwrap().unwrap();
        let continued = engine
            .load_session(&source, lbx_timer::LoadMode::AddToCurrent, at(20000), 20000)
            .unwrap();
        engine
            .update_name_notes(&continued[0], "More dice types", "Updated notes")
            .unwrap();
        let report = engine.session_report(&source, at(20000)).unwrap().unwrap();
        assert_eq!(report.total_ms(), 10000);
        assert_eq!(report.recordings.len(), 2);
        assert_eq!(report.timers[0].name, "More dice types");
        assert_eq!(
            report.timers[0].notes,
            ["Initial notes\nSecond line", "Updated notes"]
        );
        assert_eq!(
            engine.task(&first).unwrap().unwrap().notes,
            "Initial notes\nSecond line"
        );
        assert!(engine.running().is_none());
        engine.start(&continued[0], at(20000), 20000).unwrap();
        engine.start(&continued[1], at(23000), 23000).unwrap();
        engine.end_session(at(25000), 25000).unwrap();
    }
    let engine = TimerEngine::boot(db.open(), at(30000), 0).unwrap();
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    let counts = || {
        conn.query_row("SELECT (SELECT count(*) FROM sessions), (SELECT count(*) FROM tasks), (SELECT count(*) FROM events), (SELECT sum(elapsed_ms) FROM tasks)", [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?))).unwrap()
    };
    let before = counts();
    for session in engine.session_history().unwrap() {
        let report = engine
            .session_report(session.id(), at(30000))
            .unwrap()
            .unwrap();
        assert_eq!(report.total_ms(), 15000);
        assert_eq!(report.recordings.len(), 2);
        assert_eq!(report.timers[0].elapsed_ms, 8000);
        assert_eq!(report.timers[1].elapsed_ms, 7000);
        assert_eq!(report.timers[1].notes, ["Play flow"]);
    }
    assert_eq!(counts(), before);
}

#[test]
fn end_session_failure_preserves_running_timer_and_emits_nothing() {
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
    let a = engine.timers()[0].id.clone();
    let b = engine.add_timer().unwrap();
    engine.start(&a, at(0), 0).unwrap();
    engine.start(&b, at(500), 500).unwrap();
    engine.tick(at(1000), 1000).unwrap();
    let session_id = engine.session().id.clone();
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_stop BEFORE INSERT ON events WHEN NEW.kind = 'stop' BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;").unwrap();
    assert!(engine.end_session(at(1000), 1000).is_err());
    assert_eq!(engine.session().id, session_id, "session was discarded");
    assert_eq!(engine.running().unwrap().id, b);
    assert_eq!(engine.task(&b).unwrap().unwrap().state, TaskState::Running);
    assert_eq!(engine.task(&a).unwrap().unwrap().state, TaskState::Paused);
    assert_eq!(engine.events(&a).unwrap().len(), 2);
    assert_eq!(engine.events(&b).unwrap().len(), 1);
    let ended_at: Option<String> = conn
        .query_row(
            "SELECT ended_at FROM sessions WHERE id = ?1",
            [&session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(ended_at.is_none(), "session was closed despite the failure");
    conn.execute_batch("DROP TRIGGER reject_stop").unwrap();
    engine.end_session(at(2000), 2000).unwrap();
    assert_eq!(engine.task(&b).unwrap().unwrap().elapsed_ms, 1500);
    assert_eq!(engine.task(&a).unwrap().unwrap().elapsed_ms, 500);
}

#[test]
fn switch_failure_rolls_back_both_timers() {
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
    let a = engine.timers()[0].id.clone();
    let b = engine.add_timer().unwrap();
    engine.start(&a, at(0), 0).unwrap();
    engine.tick(at(1000), 1000).unwrap();
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_start BEFORE INSERT ON events WHEN NEW.kind = 'start' BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;").unwrap();
    assert!(engine.start(&b, at(1000), 1000).is_err());
    assert_eq!(engine.running().unwrap().id, a);
    assert_eq!(engine.timer(&b).unwrap().state, TimerState::Unstarted);
    assert_eq!(engine.task(&a).unwrap().unwrap().state, TaskState::Running);
    assert!(engine.task(&b).unwrap().is_none());
    assert_eq!(
        engine.events(&a).unwrap().len(),
        1,
        "partial pause was kept"
    );
    conn.execute_batch("DROP TRIGGER reject_start").unwrap();
    engine.start(&b, at(1500), 1500).unwrap();
    assert_eq!(engine.running().unwrap().id, b);
    assert_eq!(engine.task(&a).unwrap().unwrap().elapsed_ms, 1500);
}

#[test]
fn sleep_detected_while_switching_pauses_only_the_running_timer() {
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
    let a = engine.timers()[0].id.clone();
    let b = engine.add_timer().unwrap();
    let c = engine.add_timer().unwrap();
    engine.start(&a, at(0), 0).unwrap();
    engine.start(&b, at(1000), 1000).unwrap();
    // The machine slept: wall time jumped a minute, monotonic time 2 s.
    engine.start(&c, at(61000), 3000).unwrap();
    let kinds = |id: &str| {
        engine
            .events(id)
            .unwrap()
            .iter()
            .map(|e| e.kind)
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds(&a), [EventKind::Start, EventKind::Pause]);
    assert_eq!(kinds(&b), [EventKind::Start, EventKind::SleepPause]);
    assert_eq!(engine.task(&b).unwrap().unwrap().elapsed_ms, 2000);
    let c_events = engine.events(&c).unwrap();
    assert_eq!(c_events[0].kind, EventKind::Start);
    assert_eq!(
        c_events[0].cause, None,
        "nothing was running to switch from"
    );
    assert!(engine.notice().is_some(), "sleep notice was cleared");
}

#[test]
fn crash_with_a_linked_session_recovers_at_the_last_checkpoint() {
    let db = Database::new();
    let (a, b, session_id);
    {
        let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
        a = engine.timers()[0].id.clone();
        b = engine.add_timer().unwrap();
        engine.start(&a, at(0), 0).unwrap();
        engine.start(&b, at(2000), 2000).unwrap();
        engine.tick(at(3000), 3000).unwrap();
        session_id = engine.session().id.clone();
        // Simulate a crash: skip Drop, which would close the session cleanly.
        std::mem::forget(engine);
    }
    // The forgotten engine still holds the lock file handle; reopen through a
    // fresh path by copying the database files.
    let copy = Database::new();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::copy(
            format!("{}{suffix}", db.0.display()),
            format!("{}{suffix}", copy.0.display()),
        );
    }
    let engine = TimerEngine::boot(copy.open(), at(600000), 0).unwrap();
    assert!(engine.notice().is_some());
    let b_task = engine.task(&b).unwrap().unwrap();
    assert_eq!(b_task.state, TaskState::Paused);
    assert_eq!(b_task.elapsed_ms, 1000);
    assert_eq!(
        engine.events(&b).unwrap().last().unwrap().kind,
        EventKind::RecoveryPause
    );
    assert_eq!(engine.task(&a).unwrap().unwrap().elapsed_ms, 2000);
    let history = engine.session_history().unwrap();
    let session = &history[0].session;
    assert_eq!(session.id, session_id);
    assert_eq!(session.end_reason, Some(SessionEnd::Recovered));
    assert_eq!(
        session.ended_at,
        Some(at(3000)),
        "end must be the checkpoint"
    );
    assert_eq!(history[0].total_ms(), 3000);
    assert_ne!(engine.session().id, session_id);
}

#[test]
fn a_launch_without_recorded_time_writes_nothing() {
    let db = Database::new();
    {
        let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
        let b = engine.add_timer().unwrap();
        engine.update_name_notes(&b, "Draft", "").unwrap();
        engine.pause(at(1000), 1000).unwrap();
        engine.end_session(at(2000), 2000).unwrap();
        engine.pause_for_close(at(3000), 3000).unwrap();
    }
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    for table in ["tasks", "events", "sessions"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} has rows");
    }
}

#[test]
fn file_lock_prevents_a_second_timer_and_is_released() {
    let db = Database::new();
    let first = db.open();
    assert!(matches!(
        AppData::open(&db.0),
        Err(lbx_timer::AppError::AlreadyRunning(_))
    ));
    drop(first);
    assert!(AppData::open(&db.0).is_ok());
}

fn new_timer(id: &str, position: i64) -> NewTimer<'_> {
    NewTimer {
        id,
        name: "",
        notes: "",
        position,
        series_id: id,
        continues_task_id: None,
    }
}

#[test]
fn switch_events_name_the_other_timer_and_survive_its_deletion() {
    let db = Database::new();
    let mut data = db.open();
    data.switch("s", None, SwitchTarget::Start(new_timer("a", 1)), at(0))
        .unwrap();
    data.switch(
        "s",
        Some(("a", 1000)),
        SwitchTarget::Start(new_timer("b", 2)),
        at(1000),
    )
    .unwrap();
    let a_events = data.events("a").unwrap();
    let b_events = data.events("b").unwrap();
    assert_eq!(a_events[1].related_task_id.as_deref(), Some("b"));
    assert_eq!(b_events[0].related_task_id.as_deref(), Some("a"));
    assert_eq!(a_events[0].related_task_id, None);

    data.transition("b", EventKind::Pause, at(2000), 1000, None)
        .unwrap();
    data.end_session("s", at(3000), lbx_timer::SessionEnd::User, None)
        .unwrap();
    data.delete_task("b").unwrap();
    // The link is cleared; a's events themselves stay.
    let a_events = data.events("a").unwrap();
    assert_eq!(a_events.len(), 3);
    assert_eq!(a_events[1].related_task_id, None);
}

#[test]
fn storage_rejects_a_second_running_task_and_rolls_back() {
    let db = Database::new();
    let mut data = db.open();
    data.switch("s", None, SwitchTarget::Start(new_timer("first", 1)), at(0))
        .unwrap();
    // Starting another timer without pausing the first violates the
    // one-running-task rule; the whole transaction is rolled back.
    assert!(data
        .switch(
            "s",
            None,
            SwitchTarget::Start(new_timer("second", 2)),
            at(1000)
        )
        .is_err());
    assert_eq!(data.tasks().unwrap().len(), 1);
    assert!(data.task("second").unwrap().is_none());
    assert_eq!(data.events("first").unwrap().len(), 1);
}

#[test]
fn storage_rejects_a_second_open_session() {
    let db = Database::new();
    let mut data = db.open();
    data.switch("a", None, SwitchTarget::Start(new_timer("first", 1)), at(0))
        .unwrap();
    data.transition("first", EventKind::Pause, at(1000), 1000, None)
        .unwrap();
    assert!(data
        .switch(
            "b",
            None,
            SwitchTarget::Start(new_timer("second", 1)),
            at(2000)
        )
        .is_err());
    assert!(data.session("b").unwrap().is_none());
}

#[test]
fn databases_with_another_schema_version_are_refused() {
    let db = Database::new();
    {
        let conn = rusqlite::Connection::open(&db.0).unwrap();
        conn.execute_batch("CREATE TABLE tasks (id TEXT); PRAGMA user_version = 2;")
            .unwrap();
    }
    match AppData::open(&db.0) {
        Err(AppError::UnsupportedSchema {
            found, expected, ..
        }) => {
            assert_eq!(found, 2);
            assert_eq!(expected, lbx_timer::storage::SCHEMA_VERSION);
        }
        other => panic!(
            "expected an unsupported-schema error, got {:?}",
            other.err()
        ),
    }
    // A foreign file that never set a version is refused too.
    let foreign = Database::new();
    {
        let conn = rusqlite::Connection::open(&foreign.0).unwrap();
        conn.execute_batch("CREATE TABLE notes (body TEXT);")
            .unwrap();
    }
    assert!(matches!(
        AppData::open(&foreign.0),
        Err(AppError::UnsupportedSchema { found: 0, .. })
    ));
}

#[test]
fn backwards_wall_clock_does_not_change_elapsed_time() {
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(10000), 0).unwrap();
    let id = engine.timers()[0].id.clone();
    engine.start(&id, at(10000), 0).unwrap();
    engine.tick(at(5000), 1000).unwrap();
    engine.end_session(at(6000), 2000).unwrap();
    assert_eq!(engine.task(&id).unwrap().unwrap().elapsed_ms, 2000);
}

#[test]
fn deleting_saved_task_cascades_events_and_protects_the_current_session() {
    let db = Database::new();
    let mut engine = TimerEngine::boot(db.open(), at(0), 0).unwrap();
    let running = engine.timers()[0].id.clone();
    let paused = engine.add_timer().unwrap();
    let unstarted = engine.add_timer().unwrap();
    engine.start(&paused, at(0), 0).unwrap();
    engine.start(&running, at(1000), 1000).unwrap();
    for id in [&running, &paused, &unstarted] {
        assert!(engine.delete_history_task(id).is_err(), "{id} deleted");
    }
    let first_session = engine.end_session(at(2000), 2000).unwrap().unwrap();
    assert_eq!(engine.events(&running).unwrap().len(), 2);

    let other = engine.timers()[0].id.clone();
    engine.start(&other, at(3000), 3000).unwrap();
    engine.end_session(at(4000), 4000).unwrap();

    assert!(engine.delete_history_task(&running).is_ok());
    assert!(engine.task(&running).unwrap().is_none());
    assert!(engine.events(&running).unwrap().is_empty());
    assert!(engine.task(&other).unwrap().is_some());
    let sessions = engine.session_history().unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(engine.delete_history_task(&paused).is_ok());
    let sessions = engine.session_history().unwrap();
    assert_eq!(sessions.len(), 1, "empty session left in history");
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    let orphan: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            [first_session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(orphan, 0);
}

#[test]
fn a_refused_database_is_left_byte_for_byte_unchanged() {
    fn files(path: &std::path::Path) -> [Vec<u8>; 2] {
        ["", "-wal"]
            .map(|suffix| std::fs::read(format!("{}{suffix}", path.display())).unwrap_or_default())
    }

    // 1. A rollback-journal database: switching it to WAL would rewrite its
    //    header.
    let plain = Database::new();
    rusqlite::Connection::open(&plain.0)
        .unwrap()
        .execute_batch("CREATE TABLE tasks (id TEXT); PRAGMA user_version = 3;")
        .unwrap();

    // 2. A WAL database left with uncheckpointed writes and no open
    //    connection (what a crash leaves behind): opening and closing it
    //    read-write would checkpoint the WAL into the main file.
    let source = Database::new();
    let wal = Database::new();
    {
        let writer = rusqlite::Connection::open(&source.0).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA wal_autocheckpoint = 0;
                 CREATE TABLE tasks (id TEXT);
                 INSERT INTO tasks VALUES ('a'), ('b');
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        for suffix in ["", "-wal"] {
            std::fs::copy(
                format!("{}{suffix}", source.0.display()),
                format!("{}{suffix}", wal.0.display()),
            )
            .unwrap();
        }
    }

    for fixture in [&plain, &wal] {
        let before = files(&fixture.0);
        assert!(matches!(
            AppData::open(&fixture.0),
            Err(AppError::UnsupportedSchema { found: 3, .. })
        ));
        assert_eq!(
            files(&fixture.0),
            before,
            "refusing {:?} changed it",
            fixture.0
        );
    }
    assert!(!files(&wal.0)[1].is_empty(), "fixture should keep its WAL");
}
