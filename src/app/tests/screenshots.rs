//! Reproducible README captures using disposable, illustrative session data.

use super::*;

#[test]
#[ignore = "documentation capture; see docs/testing.md"]
fn readme_screenshots() {
    let db = TempDb::new("readme-screenshots");
    let base = Utc.with_ymd_and_hms(2026, 11, 17, 1, 0, 0).unwrap();
    let time = |minutes| base + chrono::Duration::minutes(minutes);
    let mono = |minutes: u64| minutes * 60_000;
    let source;
    {
        let mut engine = TimerEngine::boot(AppData::open(&db.0).unwrap(), base, 0).unwrap();
        let review = engine.timers()[0].id.clone();
        let qa = engine.add_timer().unwrap();
        let docs = engine.add_timer().unwrap();
        for (id, name, notes) in [
            (
                &review,
                "Code review",
                "Reviewed timer recovery and session links.",
            ),
            (
                &qa,
                "Testing",
                "Checked reconnects and automatic time verification.",
            ),
            (&docs, "Documentation", "Updated the setup guide."),
        ] {
            engine.update_name_notes(id, name, notes).unwrap();
        }
        engine.start(&review, time(0), mono(0)).unwrap();
        engine.start(&qa, time(20), mono(20)).unwrap();
        engine.pause(time(30), mono(30)).unwrap();
        engine.start(&docs, time(31), mono(31)).unwrap();
        source = engine.end_session(time(36), mono(36)).unwrap().unwrap();
    }

    let mut app = App::at_path(db.0.clone());
    drop(app.engine.take());
    let mut engine = TimerEngine::boot(AppData::open(&db.0).unwrap(), time(60), mono(60)).unwrap();
    let continued = engine
        .load_session(&source, LoadMode::AddToCurrent, time(60), mono(60))
        .unwrap();
    engine.start(&continued[0], time(60), mono(60)).unwrap();
    engine.start(&continued[1], time(70), mono(70)).unwrap();
    engine.pause(time(72), mono(72)).unwrap();
    engine.start(&continued[1], time(73), mono(73)).unwrap();
    engine.tick(time(76), mono(76)).unwrap();
    app.engine = Some(engine);
    app.focused_id = Some(continued[1].clone());
    app.selected_id = Some(continued[1].clone());
    app.name = "Testing".into();
    app.notes = "Checked reconnects and automatic time verification.".into();
    app.hubstaff_status = Some(HubstaffStatus::Tracking {
        project: Some("Demo project".into()),
    });
    app.hubstaff_last_success = Some(Instant::now());
    app.refresh_history();
    app.refresh_events();
    snapshot(&app, SIZE, "readme-timers");

    // Expand the completed recording without advancing the injected clock.
    app.expanded_sessions.insert(source.clone());
    app.refresh_history();
    snapshot(&app, (1280.0, 1000.0), "readme-details-history");

    app.report = Some(ReportState {
        source_id: source.clone(),
        data: super::engine(&app)
            .session_report(&source, time(76))
            .unwrap()
            .unwrap(),
        generated_at: time(76),
        unverified: false,
        show_recordings: false,
    });
    snapshot(&app, SIZE, "readme-report");
}
