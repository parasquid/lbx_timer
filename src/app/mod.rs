//! Application state and update logic. Views live in [`view`]; pure text
//! formatting in [`format`]; sizes and styles in [`metrics`] and [`style`].

mod format;
mod metrics;
mod style;
#[cfg(test)]
mod tests;
mod view;

use crate::app::metrics::*;
use chrono::{DateTime, Utc};
use iced::{time, Subscription, Task as IcedTask};
use lbx_timer::breakdown::Segment;
use lbx_timer::provider::{
    self, DiscoveryEnv, HubstaffSnapshot, HubstaffStatus, ObserverConfig, ProviderError,
    HUBSTAFF_FRESHNESS, HUBSTAFF_POLL_INTERVAL,
};
use lbx_timer::storage::timer_label;
use lbx_timer::verification::{self, TimeVerification, VerificationResult, VerificationState};
use lbx_timer::{
    default_database_path, AppData, LoadMode, LogEntry, SeriesTotal, SessionSummary,
    Task as DbTask, Timer, TimerEngine, TimerState,
};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Run the desktop app.
pub fn run() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(|_: &App| "LBX Timer".to_owned())
        .theme(|_: &App| iced::Theme::Dark)
        .subscription(App::subscription)
        .window_size(WINDOW_SIZE)
        .exit_on_close_request(false)
        // iced 0.14's GPU renderer draws multisampled canvas meshes (the row
        // icons) at stale positions inside scrolled content, leaving fixed
        // and duplicated icons. Text and quads antialias on their own, so
        // turning MSAA off only affects the icons' triangle edges.
        .antialiasing(false)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    HubstaffVerificationChecked(Vec<VerificationResult>),
    HubstaffPolled {
        request_id: u64,
        start_intent: Option<u64>,
        result: Result<HubstaffSnapshot, ProviderError>,
    },
    CloseRequested(iced::window::Id),
    /// Start or resume a timer, pausing the one that is running.
    Start(String),
    /// Pause the running timer.
    Pause,
    EndSession,
    AddTimer,
    RemoveTimer(String),
    /// Show a timer of the current session in the clock card and Details.
    FocusTimer(String),
    /// Select a stored task from history for Details.
    SelectTask(String),
    ToggleSession(String),
    /// Expand a linked session in history.
    RevealSession(String),
    /// Resume an earlier session: continue its timers in the current session.
    ResumeSession(String),
    ConfirmResume(LoadMode),
    CancelResume,
    OpenReport(String),
    RefreshReport,
    CloseReport,
    CopyReport,
    ToggleReportRecordings,
    NameChanged(String),
    NotesChanged(String),
    SaveDetails,
    ToggleDetails,
    ToggleHistory,
    DismissNotice,
}

struct App {
    engine: Option<TimerEngine>,
    startup_error: Option<String>,
    error_message: Option<String>,
    notice: Option<String>,
    history: Vec<SessionSummary>,
    /// Segments of expanded history sessions, keyed by session key.
    breakdowns: HashMap<String, Vec<Segment>>,
    expanded_sessions: HashSet<String>,
    /// Timer of the current session shown in the clock card.
    focused_id: Option<String>,
    /// Timer or stored task edited in Details.
    selected_id: Option<String>,
    /// Readable log of the selected timer's events.
    selected_log: Vec<LogEntry>,
    /// Time of each piece of work across sessions, keyed by series.
    series_totals: HashMap<String, SeriesTotal>,
    name: String,
    notes: String,
    details_expanded: bool,
    history_expanded: bool,
    /// Session waiting for the user to choose how to resume it.
    pending_resume: Option<String>,
    report: Option<ReportState>,
    monotonic_start: Instant,
    observer_config: ObserverConfig,
    hubstaff_status: Option<HubstaffStatus>,
    hubstaff_last_success: Option<Instant>,
    hubstaff_verified_checkpoint: Option<(String, DateTime<Utc>, i64)>,
    time_verifications: Vec<TimeVerification>,
    verification_check_pending: bool,
    verification_last_check: Option<Instant>,
    hubstaff_audit_path: Option<std::path::PathBuf>,
    hubstaff_poll_pending: bool,
    hubstaff_poll_id: u64,
    hubstaff_last_poll_started: Option<Instant>,
    hubstaff_ready: bool,
    hubstaff_poll_failures: u32,
    hubstaff_retry_after: Option<Instant>,
    pending_start: Option<(String, u64)>,
    start_intent_generation: u64,
    /// Session that may start once when Hubstaff is first confirmed tracking.
    hubstaff_startup_follow: Option<String>,
    hubstaff_pause_retry: bool,
    hubstaff_pause_reason: Option<String>,
    hubstaff_pause_boundary: Option<(DateTime<Utc>, u64)>,
    hubstaff_resume_candidate: Option<HubstaffResumeCandidate>,
    hubstaff_resume_target: Option<HubstaffResumeTarget>,
}

#[derive(Debug, Clone)]
struct HubstaffResumeCandidate {
    session_id: String,
    timer_id: String,
}

struct ReportState {
    source_id: String,
    data: lbx_timer::report::SessionReport,
    generated_at: DateTime<Utc>,
    unverified: bool,
    show_recordings: bool,
}

#[derive(Debug, Clone)]
struct HubstaffResumeTarget {
    session_id: String,
    timer_id: String,
    armed_after_poll_id: u64,
}

impl App {
    pub(crate) fn new() -> Self {
        let config = provider::discover(DiscoveryEnv::from_process());
        Self::at_path_with_observer(database_path_from_args(), config)
    }

    #[cfg(test)]
    pub(crate) fn at_path(path: std::path::PathBuf) -> Self {
        Self::at_path_with_observer(path, ObserverConfig::Standalone)
    }

    pub(crate) fn at_path_with_observer(
        path: std::path::PathBuf,
        observer_config: ObserverConfig,
    ) -> Self {
        let hubstaff_status = match &observer_config {
            ObserverConfig::Standalone => None,
            ObserverConfig::Hubstaff(_) => Some(HubstaffStatus::Checking),
            ObserverConfig::Unavailable(reason) => Some(HubstaffStatus::Unavailable {
                reason: reason.clone(),
            }),
        };
        let mut app = Self {
            engine: None,
            startup_error: None,
            error_message: None,
            notice: None,
            history: Vec::new(),
            breakdowns: HashMap::new(),
            expanded_sessions: HashSet::new(),
            focused_id: None,
            selected_id: None,
            selected_log: Vec::new(),
            series_totals: HashMap::new(),
            name: String::new(),
            notes: String::new(),
            details_expanded: false,
            history_expanded: false,
            pending_resume: None,
            report: None,
            monotonic_start: Instant::now(),
            observer_config,
            hubstaff_status,
            hubstaff_last_success: None,
            hubstaff_verified_checkpoint: None,
            time_verifications: Vec::new(),
            verification_check_pending: false,
            verification_last_check: None,
            hubstaff_audit_path: verification::audit_log_path(),
            hubstaff_poll_pending: false,
            hubstaff_poll_id: 0,
            hubstaff_last_poll_started: None,
            hubstaff_ready: false,
            hubstaff_poll_failures: 0,
            hubstaff_retry_after: None,
            pending_start: None,
            start_intent_generation: 0,
            hubstaff_startup_follow: None,
            hubstaff_pause_retry: false,
            hubstaff_pause_reason: None,
            hubstaff_pause_boundary: None,
            hubstaff_resume_candidate: None,
            hubstaff_resume_target: None,
        };
        let booted = AppData::open(&path)
            .map_err(lbx_timer::TimerError::from)
            .and_then(|db| TimerEngine::boot(db, Utc::now(), app.mono_ms()));
        match booted {
            Ok(engine) => {
                app.notice = engine.notice().map(str::to_owned);
                app.engine = Some(engine);
                if matches!(&app.observer_config, ObserverConfig::Hubstaff(_)) {
                    app.hubstaff_startup_follow = app.current_session_id().map(str::to_owned);
                }
                app.refresh_history();
                if let Some(id) = app.focused_timer().map(|timer| timer.id.clone()) {
                    app.focus(id);
                }
                app.refresh_events();
            }
            Err(error) => {
                app.startup_error = Some(format!("Could not open {}\n\n{error}", path.display()));
            }
        }
        app
    }

    pub(crate) fn mono_ms(&self) -> u64 {
        self.monotonic_start
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }

    pub(crate) fn now_and_mono(&self) -> (DateTime<Utc>, u64) {
        (Utc::now(), self.mono_ms())
    }

    /// The timer shown in the clock card: the focused one if it is still in
    /// the session, otherwise the running one, otherwise the first.
    pub(crate) fn focused_timer(&self) -> Option<&Timer> {
        let engine = self.engine.as_ref()?;
        self.focused_id
            .as_deref()
            .and_then(|id| engine.timer(id))
            .or_else(|| engine.running())
            .or_else(|| engine.timers().first())
    }

    pub(crate) fn current_session_id(&self) -> Option<&str> {
        self.engine
            .as_ref()
            .map(|engine| engine.session().id.as_str())
    }

    pub(crate) fn in_current_session(&self, task_id: &str) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.session().contains(task_id))
    }

    pub(crate) fn history_session(&self, session_id: &str) -> Option<&SessionSummary> {
        self.history
            .iter()
            .find(|summary| summary.id() == session_id)
    }

    pub(crate) fn history_task(&self, task_id: &str) -> Option<&DbTask> {
        self.history
            .iter()
            .flat_map(|summary| summary.tasks.iter())
            .find(|task| task.id == task_id)
    }

    pub(crate) fn refresh_history(&mut self) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        match engine.time_verifications() {
            Ok(intervals) => self.time_verifications = intervals,
            Err(error) => self.error_message = Some(error.to_string()),
        }
        match engine.series_totals() {
            Ok(totals) => self.series_totals = totals,
            Err(error) => self.error_message = Some(error.to_string()),
        }
        match engine.session_history() {
            Ok(history) => self.history = history,
            Err(error) => {
                self.error_message = Some(error.to_string());
                return;
            }
        }
        self.breakdowns.clear();
        for summary in &self.history {
            let id = summary.id();
            if !self.expanded_sessions.contains(id) {
                continue;
            }
            match engine.session_segments(id) {
                Ok(segments) => {
                    self.breakdowns.insert(id.to_owned(), segments);
                }
                Err(error) => self.error_message = Some(error.to_string()),
            }
        }
        let live: HashSet<&str> = self.history.iter().map(SessionSummary::id).collect();
        self.expanded_sessions
            .retain(|id| live.contains(id.as_str()));
    }

    /// Load the name and notes of the selected timer or task into the form.
    /// Its event log is loaded by [`Self::refresh_events`].
    pub(crate) fn load_selected(&mut self) {
        let Some(id) = self.selected_id.clone() else {
            self.name.clear();
            self.notes.clear();
            return;
        };
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        if let Some(timer) = engine.timer(&id) {
            self.name = timer.name.clone();
            self.notes = timer.notes.clone();
        } else if let Ok(Some(task)) = engine.task(&id) {
            self.name = task.name.clone();
            self.notes = task.notes.clone();
        }
    }

    /// "Timer N" for the selected timer or stored task.
    pub(crate) fn default_name(&self) -> String {
        let Some(id) = self.selected_id.as_deref() else {
            return "Name".to_owned();
        };
        let engine = self.engine.as_ref();
        engine
            .and_then(|engine| engine.timer(id))
            .map(|timer| timer_label("", timer.number))
            .or_else(|| {
                self.history_task(id)
                    .map(|task| timer_label("", task.position))
            })
            .unwrap_or_else(|| "Name".to_owned())
    }

    pub(crate) fn focus(&mut self, timer_id: String) {
        self.focused_id = Some(timer_id.clone());
        self.selected_id = Some(timer_id);
        self.load_selected();
    }

    pub(crate) fn set_error(&mut self, error: impl std::fmt::Display) {
        self.error_message = Some(error.to_string());
    }

    pub(crate) fn save_details(&mut self) -> bool {
        let Some(task_id) = self.selected_id.clone() else {
            return true;
        };
        let Some(engine) = self.engine.as_mut() else {
            return true;
        };
        // Only timers of the current session are editable; past timers are
        // a record and shown read-only.
        if engine.timer(&task_id).is_none() {
            return true;
        }
        match engine.update_name_notes(&task_id, &self.name, &self.notes) {
            Ok(()) => {
                self.error_message = None;
                true
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                false
            }
        }
    }

    pub(crate) fn refresh_events(&mut self) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let Some(id) = self.selected_id.as_deref() else {
            self.selected_log.clear();
            return;
        };
        match engine.event_log(id) {
            Ok(log) => self.selected_log = log,
            Err(error) => self.error_message = Some(error.to_string()),
        }
    }

    fn open_report(&mut self, source_id: String, show_recordings: bool) {
        self.tick();
        self.refresh_history();
        let generated_at = Utc::now();
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        match engine.session_report(&source_id, generated_at) {
            Ok(Some(data)) => {
                let unverified = self.time_verifications.iter().any(|interval| {
                    interval.state == VerificationState::Unverified
                        && data.contains_task(&interval.task_id)
                });
                self.report = Some(ReportState {
                    source_id,
                    data,
                    generated_at,
                    unverified,
                    show_recordings,
                });
            }
            Ok(None) => self.set_error("The selected session is no longer available."),
            Err(error) => self.set_error(error),
        }
    }

    fn report_text(&self) -> Option<String> {
        self.report.as_ref().map(|report| {
            format::session_report_text(&report.data, report.generated_at, report.unverified)
        })
    }

    /// Show the current session's first timer, with nothing left over from
    /// what was shown before: used after the session changes.
    pub(crate) fn reset_to_current_session(&mut self) {
        self.cancel_local_hubstaff_automation();
        self.pending_resume = None;
        self.notice = None;
        if let Some(engine) = self.engine.as_mut() {
            engine.clear_notice();
        }
        let first = self
            .engine
            .as_ref()
            .and_then(|engine| engine.timers().first())
            .map(|timer| timer.id.clone());
        self.focused_id = None;
        self.selected_id = None;
        match first {
            Some(id) => self.focus(id),
            None => self.load_selected(),
        }
    }

    pub(crate) fn load_into_session(&mut self, session_id: String, mode: LoadMode) {
        self.cancel_pending_start();
        self.cancel_local_hubstaff_automation();
        if !self.flush_pending_hubstaff_pause() {
            return;
        }
        if !self.save_details() {
            return;
        }
        let mut loaded = Vec::new();
        let ok = self.run(|engine, now, mono| {
            loaded = engine.load_session(&session_id, mode, now, mono)?;
            Ok(())
        });
        if ok {
            self.reset_to_current_session();
            if let Some(id) = loaded.first() {
                self.focus(id.clone());
            }
        }
        self.refresh_history();
    }

    /// Run a timer action, report its error, and clear a stale one.
    pub(crate) fn run(
        &mut self,
        action: impl FnOnce(&mut TimerEngine, DateTime<Utc>, u64) -> Result<(), lbx_timer::TimerError>,
    ) -> bool {
        let (now, mono) = self.now_and_mono();
        let Some(engine) = self.engine.as_mut() else {
            return false;
        };
        let result = action(engine, now, mono);
        let notice = engine.notice().map(str::to_owned);
        self.notice = notice;
        match result {
            Ok(()) => {
                self.error_message = None;
                true
            }
            Err(error) => {
                self.set_error(error);
                false
            }
        }
    }

    /// Whether Details edits the selected timer: only timers of the current
    /// session can be changed. Past timers are shown read-only.
    pub(crate) fn details_editable(&self) -> bool {
        self.selected_id
            .as_deref()
            .is_some_and(|id| self.in_current_session(id))
    }

    pub(crate) fn hubstaff_status(&self) -> Option<&HubstaffStatus> {
        self.hubstaff_status.as_ref()
    }

    pub(crate) fn can_start_timer(&self) -> bool {
        match &self.observer_config {
            ObserverConfig::Standalone => true,
            ObserverConfig::Unavailable(_) => false,
            ObserverConfig::Hubstaff(_) => {
                matches!(self.hubstaff_status, Some(HubstaffStatus::Tracking { .. }))
                    && self
                        .hubstaff_last_success
                        .is_some_and(|updated| updated.elapsed() <= HUBSTAFF_FRESHNESS)
                    && !self.hubstaff_pause_retry
                    && self.pending_start.is_none()
            }
        }
    }

    fn cancel_pending_start(&mut self) {
        self.start_intent_generation = self.start_intent_generation.wrapping_add(1);
        self.pending_start = None;
    }

    fn begin_start_probe(&mut self, timer_id: String) -> IcedTask<Message> {
        self.cancel_pending_start();
        let intent = self.start_intent_generation;
        self.pending_start = Some((timer_id, intent));
        self.hubstaff_status = Some(HubstaffStatus::Checking);
        self.begin_hubstaff_poll(Some(intent))
    }

    fn begin_hubstaff_poll(&mut self, start_intent: Option<u64>) -> IcedTask<Message> {
        let ObserverConfig::Hubstaff(path) = &self.observer_config else {
            return IcedTask::none();
        };
        if self.hubstaff_poll_pending {
            return IcedTask::none();
        }
        let path = path.clone();
        self.hubstaff_poll_pending = true;
        self.hubstaff_poll_id = self.hubstaff_poll_id.wrapping_add(1);
        self.hubstaff_last_poll_started = Some(Instant::now());
        if start_intent.is_some() {
            self.hubstaff_status = Some(HubstaffStatus::Checking);
        }
        let request_id = self.hubstaff_poll_id;
        let check_readiness = !self.hubstaff_ready;
        IcedTask::perform(
            async move {
                if check_readiness {
                    provider::poll(&path).await
                } else {
                    provider::poll_status(&path).await
                }
            },
            move |result| Message::HubstaffPolled {
                request_id,
                start_intent,
                result,
            },
        )
    }

    fn poll_hubstaff_if_due(&mut self) -> IcedTask<Message> {
        if self.hubstaff_poll_pending
            || !matches!(self.observer_config, ObserverConfig::Hubstaff(_))
        {
            return IcedTask::none();
        }
        if self.pending_start.is_none()
            && self
                .hubstaff_retry_after
                .is_some_and(|deadline| Instant::now() < deadline)
        {
            return IcedTask::none();
        }
        let due = self
            .hubstaff_last_poll_started
            .is_none_or(|started| started.elapsed() >= HUBSTAFF_POLL_INTERVAL);
        if !due {
            return IcedTask::none();
        }
        let pending_intent = self.pending_start.as_ref().map(|(_, intent)| *intent);
        self.begin_hubstaff_poll(pending_intent)
    }

    fn handle_hubstaff_poll(
        &mut self,
        request_id: u64,
        start_intent: Option<u64>,
        result: Result<HubstaffSnapshot, ProviderError>,
    ) -> IcedTask<Message> {
        if request_id != self.hubstaff_poll_id || !self.hubstaff_poll_pending {
            return IcedTask::none();
        }
        self.hubstaff_poll_pending = false;
        if result.is_ok()
            && self
                .time_verifications
                .iter()
                .any(|interval| interval.ended_at.is_none())
        {
            self.verification_last_check = None;
        }
        let approved_tracking = result.as_ref().is_ok_and(|snapshot| snapshot.tracking);
        let pause_reason = match result {
            Ok(snapshot) => {
                self.hubstaff_ready = true;
                self.hubstaff_poll_failures = 0;
                self.hubstaff_retry_after = None;
                self.hubstaff_last_success = Some(Instant::now());
                if snapshot.tracking {
                    self.hubstaff_status = Some(HubstaffStatus::Tracking {
                        project: snapshot.project,
                    });
                    None
                } else {
                    self.hubstaff_status = Some(HubstaffStatus::Stopped {
                        project: snapshot.project,
                    });
                    if start_intent.is_none() && self.pending_start.is_none() {
                        self.remember_hubstaff_resume_candidate();
                    }
                    Some("Hubstaff reported tracking stopped.".to_owned())
                }
            }
            Err(error) => {
                self.hubstaff_ready = false;
                self.hubstaff_poll_failures = self.hubstaff_poll_failures.saturating_add(1);
                self.hubstaff_retry_after =
                    Some(Instant::now() + provider::retry_delay(self.hubstaff_poll_failures));
                self.hubstaff_last_success = None;
                let reason = error.0;
                self.hubstaff_status = Some(HubstaffStatus::Unavailable {
                    reason: reason.clone(),
                });
                self.mark_hubstaff_unverified(&reason);
                None
            }
        };

        if let Some(reason) = pause_reason {
            self.pause_for_hubstaff(&reason);
        }

        if self.hubstaff_last_success.is_some() {
            if approved_tracking && !self.hubstaff_pause_retry {
                let (now, mono) = self.now_and_mono();
                if let Some(engine) = self.engine.as_mut() {
                    match engine.verified_checkpoint(now, mono) {
                        Ok(checkpoint) => self.hubstaff_verified_checkpoint = checkpoint,
                        Err(error) => self.set_error(error),
                    }
                }
            } else {
                self.hubstaff_verified_checkpoint = None;
            }
        }

        let start_matches = self
            .pending_start
            .as_ref()
            .is_some_and(|(_, intent)| Some(*intent) == start_intent);
        if start_matches {
            if approved_tracking && !self.hubstaff_pause_retry {
                if let Some((timer_id, _)) = self.pending_start.take() {
                    if self.in_current_session(&timer_id) {
                        let id = timer_id.clone();
                        if self.run(move |engine, now, mono| engine.start(&id, now, mono)) {
                            self.focus(timer_id);
                            self.refresh_history();
                        }
                    }
                }
            } else {
                self.pending_start = None;
            }
        } else if let Some((_, pending_intent)) = self.pending_start.as_ref() {
            // An older poll cannot authorize a Start click made after it began.
            let pending_intent = *pending_intent;
            self.hubstaff_status = Some(HubstaffStatus::Checking);
            self.refresh_events();
            return self.begin_hubstaff_poll(Some(pending_intent));
        }

        if start_intent.is_none() && approved_tracking {
            self.follow_hubstaff_startup();
            self.resume_hubstaff_target(request_id);
        }
        if approved_tracking {
            if let Some(timer) = self.engine.as_ref().and_then(|engine| engine.running()) {
                if self
                    .hubstaff_verified_checkpoint
                    .as_ref()
                    .is_none_or(|(id, _, _)| *id != timer.id)
                {
                    self.hubstaff_verified_checkpoint =
                        Some((timer.id.clone(), Utc::now(), timer.raw_elapsed_ms()));
                }
            }
        }
        self.refresh_history();
        self.refresh_events();
        self.check_time_verifications()
    }

    fn mark_hubstaff_unverified(&mut self, reason: &str) {
        if self.hubstaff_pause_retry {
            return;
        }
        let (now, mono) = self.now_and_mono();
        if let Some(engine) = self.engine.as_mut() {
            if let Err(error) = engine.mark_time_unverified(
                self.hubstaff_verified_checkpoint.as_ref(),
                reason,
                now,
                mono,
            ) {
                // If uncertainty cannot be saved, retain the existing durable
                // pause/retry boundary rather than counting unmarked time.
                self.remember_hubstaff_resume_candidate();
                self.pause_for_hubstaff("Could not save the unverified interval.");
                self.set_error(error);
            }
        }
        self.refresh_history();
        self.refresh_events();
    }

    pub(crate) fn verification_label(&self, task_id: Option<&str>) -> Option<&'static str> {
        let intervals = || {
            self.time_verifications
                .iter()
                .filter(|interval| task_id.is_none_or(|id| interval.task_id == id))
        };
        if intervals().any(|interval| interval.state == VerificationState::Unverified) {
            Some("Unverified")
        } else {
            None
        }
    }

    fn check_time_verifications(&mut self) -> IcedTask<Message> {
        if self.verification_check_pending
            || !matches!(self.observer_config, ObserverConfig::Hubstaff(_))
            || self
                .hubstaff_last_success
                .is_none_or(|at| at.elapsed() > HUBSTAFF_FRESHNESS)
            || self
                .verification_last_check
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(5))
        {
            return IcedTask::none();
        }
        let tracking = match self.hubstaff_status {
            Some(HubstaffStatus::Tracking { .. }) => true,
            Some(HubstaffStatus::Stopped { .. }) => false,
            _ => return IcedTask::none(),
        };
        let intervals: Vec<_> = self
            .time_verifications
            .iter()
            .filter(|interval| {
                interval.ended_at.is_some() && interval.state == VerificationState::Unverified
            })
            .cloned()
            .collect();
        if intervals.is_empty() {
            return IcedTask::none();
        }
        self.verification_check_pending = true;
        self.verification_last_check = Some(Instant::now());
        let path = self.hubstaff_audit_path.clone();
        IcedTask::perform(
            verification::check_audit(path, intervals, Utc::now(), tracking),
            Message::HubstaffVerificationChecked,
        )
    }

    fn cancel_hubstaff_auto_resume(&mut self) {
        self.hubstaff_resume_candidate = None;
        self.hubstaff_resume_target = None;
    }

    fn cancel_local_hubstaff_automation(&mut self) {
        self.cancel_hubstaff_auto_resume();
        self.hubstaff_startup_follow = None;
    }

    fn remember_hubstaff_resume_candidate(&mut self) {
        if self.hubstaff_pause_retry
            || self.hubstaff_resume_candidate.is_some()
            || self.hubstaff_resume_target.is_some()
        {
            return;
        }
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let Some(timer) = engine.running() else {
            return;
        };
        self.hubstaff_resume_candidate = Some(HubstaffResumeCandidate {
            session_id: engine.session().id.clone(),
            timer_id: timer.id.clone(),
        });
    }

    fn arm_hubstaff_resume_candidate(&mut self) {
        let Some(candidate) = self.hubstaff_resume_candidate.take() else {
            return;
        };
        let is_paused_in_session = self.engine.as_ref().is_some_and(|engine| {
            engine.session().id == candidate.session_id
                && engine
                    .timer(&candidate.timer_id)
                    .is_some_and(|timer| timer.state == TimerState::Paused)
        });
        if is_paused_in_session {
            // The engine can detect system sleep while saving this pause.
            // Sleep needs local confirmation, even if Hubstaff also went stale.
            let events = match self.engine.as_ref().unwrap().events(&candidate.timer_id) {
                Ok(events) => events,
                Err(error) => {
                    self.set_error(error);
                    return;
                }
            };
            if !events
                .last()
                .is_some_and(|event| event.kind == lbx_timer::EventKind::Pause)
            {
                return;
            }
            self.hubstaff_resume_target = Some(HubstaffResumeTarget {
                session_id: candidate.session_id,
                timer_id: candidate.timer_id,
                armed_after_poll_id: self.hubstaff_poll_id,
            });
        }
    }

    fn resume_hubstaff_target(&mut self, request_id: u64) {
        let eligible = self
            .hubstaff_resume_target
            .as_ref()
            .is_some_and(|target| request_id != target.armed_after_poll_id);
        if !eligible || self.hubstaff_pause_retry || self.pending_start.is_some() {
            return;
        }
        let Some(target) = self.hubstaff_resume_target.take() else {
            return;
        };
        let is_resumable_in_session = self.engine.as_ref().is_some_and(|engine| {
            engine.session().id == target.session_id
                && engine.timer(&target.timer_id).is_some_and(|timer| {
                    matches!(timer.state, TimerState::Paused | TimerState::Unstarted)
                })
        });
        if !is_resumable_in_session {
            return;
        }
        let timer_id = target.timer_id;
        let id = timer_id.clone();
        if self.run(move |engine, now, mono| {
            engine.start_with_reason(
                &id,
                now,
                mono,
                "Triggered by Hubstaff: tracking confirmed after an automatic pause.",
            )
        }) {
            self.focus(timer_id);
            self.refresh_history();
        }
    }

    fn arm_hubstaff_selected_target(&mut self, timer_id: &str) {
        let is_fresh_stopped = matches!(
            self.hubstaff_status.as_ref(),
            Some(HubstaffStatus::Stopped { .. })
        ) && self
            .hubstaff_last_success
            .is_some_and(|updated| updated.elapsed() <= HUBSTAFF_FRESHNESS);
        if !matches!(&self.observer_config, ObserverConfig::Hubstaff(_)) || !is_fresh_stopped {
            return;
        }
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let Some(timer) = engine
            .timer(timer_id)
            .filter(|timer| matches!(timer.state, TimerState::Paused | TimerState::Unstarted))
        else {
            return;
        };
        self.hubstaff_resume_target = Some(HubstaffResumeTarget {
            session_id: engine.session().id.clone(),
            timer_id: timer.id.clone(),
            armed_after_poll_id: self.hubstaff_poll_id,
        });
    }

    fn follow_hubstaff_startup(&mut self) {
        let Some(session_id) = self.hubstaff_startup_follow.take() else {
            return;
        };
        if !self.can_start_timer() {
            self.hubstaff_startup_follow = Some(session_id);
            return;
        }
        if !self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.session().id == session_id)
        {
            return;
        }
        let Some(timer_id) = self
            .focused_timer()
            .filter(|timer| timer.state == TimerState::Unstarted)
            .map(|timer| timer.id.clone())
        else {
            return;
        };
        let id = timer_id.clone();
        if self.run(move |engine, now, mono| {
            engine.start_with_reason(
                &id,
                now,
                mono,
                "Triggered by Hubstaff: tracking confirmed at launch.",
            )
        }) {
            self.focus(timer_id);
            self.refresh_history();
        }
    }

    fn pause_for_hubstaff(&mut self, reason: &str) {
        let is_running = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.running().is_some());
        if !is_running && !self.hubstaff_pause_retry {
            return;
        }
        if !is_running {
            self.hubstaff_pause_retry = false;
            self.hubstaff_pause_reason = None;
            self.hubstaff_pause_boundary = None;
            return;
        }
        if self.hubstaff_pause_reason.is_none() {
            self.hubstaff_pause_reason = Some(format!(
                "Triggered by Hubstaff. {} The timer paused when this state was observed; polling and CLI response time can delay detection.",
                reason
            ));
        }
        if self.hubstaff_pause_boundary.is_none() {
            self.hubstaff_pause_boundary = Some(self.now_and_mono());
        }
        let (detected_at, detected_mono) = *self
            .hubstaff_pause_boundary
            .as_ref()
            .expect("Hubstaff pause boundary is set");
        let details = self.hubstaff_pause_reason.clone().unwrap_or_default();
        if self.run(|engine, _, _| {
            engine.pause_with_reason(detected_at, detected_mono, Some(&details))
        }) {
            self.hubstaff_pause_retry = false;
            self.hubstaff_pause_reason = None;
            self.hubstaff_pause_boundary = None;
            self.arm_hubstaff_resume_candidate();
            self.refresh_history();
            self.refresh_events();
        } else {
            self.hubstaff_pause_retry = true;
        }
    }

    fn flush_pending_hubstaff_pause(&mut self) -> bool {
        if !self.hubstaff_pause_retry {
            return true;
        }
        let reason = self
            .hubstaff_pause_reason
            .clone()
            .unwrap_or_else(|| "Hubstaff pause is still waiting to be saved.".to_owned());
        self.pause_for_hubstaff(&reason);
        !self.hubstaff_pause_retry
    }

    fn expire_hubstaff_state(&mut self) -> bool {
        let stale = self
            .hubstaff_last_success
            .is_some_and(|updated| updated.elapsed() > HUBSTAFF_FRESHNESS);
        if !stale || !matches!(self.observer_config, ObserverConfig::Hubstaff(_)) {
            return false;
        }
        self.hubstaff_last_success = None;
        self.hubstaff_ready = false;
        let reason = "Hubstaff status is stale.".to_owned();
        self.hubstaff_status = Some(HubstaffStatus::Unavailable {
            reason: reason.clone(),
        });
        self.mark_hubstaff_unverified(&reason);
        true
    }

    pub(crate) fn update(&mut self, message: Message) -> IcedTask<Message> {
        match message {
            Message::CloseRequested(id) => {
                self.cancel_pending_start();
                self.cancel_local_hubstaff_automation();
                if !self.flush_pending_hubstaff_pause() {
                    return IcedTask::none();
                }
                if !self.save_details() {
                    return IcedTask::none();
                }
                if !self.run(|engine, now, mono| engine.pause_for_close(now, mono)) {
                    return IcedTask::none();
                }
                return iced::window::close(id);
            }
            Message::Tick => {
                self.tick();
                return IcedTask::batch([
                    self.poll_hubstaff_if_due(),
                    self.check_time_verifications(),
                ]);
            }
            Message::HubstaffVerificationChecked(results) => {
                self.verification_check_pending = false;
                if let Some(engine) = self.engine.as_mut() {
                    if let Err(error) = engine.save_verification_results(&results) {
                        self.set_error(error);
                    }
                }
                self.refresh_history();
            }
            Message::HubstaffPolled {
                request_id,
                start_intent,
                result,
            } => {
                return self.handle_hubstaff_poll(request_id, start_intent, result);
            }
            Message::Start(timer_id) => {
                self.cancel_local_hubstaff_automation();
                if !self.save_details() {
                    return IcedTask::none();
                }
                match &self.observer_config {
                    ObserverConfig::Hubstaff(_) => {
                        let task = self.begin_start_probe(timer_id);
                        self.refresh_events();
                        return task;
                    }
                    ObserverConfig::Unavailable(_) => {
                        self.cancel_pending_start();
                        return IcedTask::none();
                    }
                    ObserverConfig::Standalone => {}
                }
                let id = timer_id.clone();
                if self.run(move |engine, now, mono| engine.start(&id, now, mono)) {
                    self.focus(timer_id);
                }
                self.refresh_history();
            }
            Message::Pause => {
                self.cancel_pending_start();
                self.cancel_local_hubstaff_automation();
                if !self.flush_pending_hubstaff_pause() {
                    return IcedTask::none();
                }
                self.run(|engine, now, mono| engine.pause(now, mono));
                self.refresh_history();
            }
            Message::EndSession => {
                self.cancel_pending_start();
                self.cancel_local_hubstaff_automation();
                if !self.flush_pending_hubstaff_pause() {
                    return IcedTask::none();
                }
                if !self.save_details() {
                    return IcedTask::none();
                }
                if self.run(|engine, now, mono| engine.end_session(now, mono).map(|_| ())) {
                    // A fresh session: Details, notices, and pending dialogs
                    // about the ended one no longer apply.
                    self.reset_to_current_session();
                }
                self.refresh_history();
            }
            Message::AddTimer => {
                self.cancel_pending_start();
                let result = self.engine.as_mut().map(TimerEngine::add_timer);
                if let Some(Err(error)) = result {
                    self.set_error(error);
                }
            }
            Message::RemoveTimer(timer_id) => {
                self.cancel_local_hubstaff_automation();
                if self.pending_start.is_some() {
                    self.cancel_pending_start();
                }
                let result = self
                    .engine
                    .as_mut()
                    .map(|engine| engine.remove_timer(&timer_id));
                match result {
                    Some(Ok(())) => {
                        if self.focused_id.as_deref() == Some(timer_id.as_str()) {
                            self.focused_id = None;
                            if let Some(id) = self.focused_timer().map(|timer| timer.id.clone()) {
                                self.focus(id);
                            }
                        } else if self.selected_id.as_deref() == Some(timer_id.as_str()) {
                            self.selected_id = self.focused_id.clone();
                            self.load_selected();
                        }
                    }
                    Some(Err(error)) => self.set_error(error),
                    None => {}
                }
            }
            Message::FocusTimer(timer_id) => {
                self.cancel_pending_start();
                self.cancel_local_hubstaff_automation();
                if self.save_details() && self.in_current_session(&timer_id) {
                    self.focus(timer_id.clone());
                    self.arm_hubstaff_selected_target(&timer_id);
                }
            }
            Message::SelectTask(task_id) => {
                self.cancel_pending_start();
                self.cancel_local_hubstaff_automation();
                if self.save_details() {
                    if self.in_current_session(&task_id) {
                        self.focus(task_id.clone());
                        self.arm_hubstaff_selected_target(&task_id);
                    } else if self.history_task(&task_id).is_some() {
                        self.selected_id = Some(task_id);
                        self.load_selected();
                    }
                }
            }
            Message::RevealSession(id) => {
                self.expanded_sessions.insert(id);
                self.refresh_history();
            }
            Message::ResumeSession(id) => {
                self.cancel_pending_start();
                self.cancel_local_hubstaff_automation();
                let has_time = self
                    .engine
                    .as_ref()
                    .is_some_and(|engine| engine.session().has_recorded_time());
                if has_time {
                    // Ask whether to add to or end the current session.
                    self.pending_resume = Some(id);
                } else {
                    self.load_into_session(id, LoadMode::AddToCurrent);
                }
            }
            Message::ConfirmResume(mode) => {
                self.cancel_pending_start();
                self.cancel_local_hubstaff_automation();
                if let Some(id) = self.pending_resume.take() {
                    self.load_into_session(id, mode);
                }
            }
            Message::CancelResume => {
                self.pending_resume = None;
            }
            Message::OpenReport(source_id) => self.open_report(source_id, false),
            Message::RefreshReport => {
                if let Some(report) = self.report.take() {
                    self.open_report(report.source_id, report.show_recordings);
                }
            }
            Message::CloseReport => self.report = None,
            Message::CopyReport => {
                if let Some(text) = self.report_text() {
                    return iced::clipboard::write(text);
                }
            }
            Message::ToggleReportRecordings => {
                if let Some(report) = self.report.as_mut() {
                    report.show_recordings = !report.show_recordings;
                }
            }
            Message::ToggleSession(key) => {
                if !self.expanded_sessions.remove(&key) {
                    self.expanded_sessions.insert(key);
                }
                self.refresh_history();
            }
            Message::NameChanged(value) => self.edit_details(|app| app.name = value),
            Message::NotesChanged(value) => self.edit_details(|app| app.notes = value),
            Message::SaveDetails => {
                if self.save_details() {
                    self.load_selected();
                    self.refresh_history();
                }
            }
            Message::ToggleDetails => {
                self.details_expanded = !self.details_expanded;
            }
            Message::ToggleHistory => {
                self.history_expanded = !self.history_expanded;
                if self.history_expanded {
                    self.refresh_history();
                }
            }
            Message::DismissNotice => {
                self.notice = None;
                if let Some(engine) = self.engine.as_mut() {
                    engine.clear_notice();
                }
            }
        }
        self.refresh_events();
        IcedTask::none()
    }

    /// Advance the running timer. Only what a tick can change is refreshed:
    /// an idle app does no work, and the event log is reloaded only when the
    /// tick recorded an event (a pause after sleep).
    fn tick(&mut self) {
        let expired = self.expire_hubstaff_state();
        if !expired {
            match self.hubstaff_status.as_ref() {
                Some(HubstaffStatus::Stopped { .. }) => {
                    self.pause_for_hubstaff("Hubstaff reports tracking stopped.");
                }
                _ if self.hubstaff_pause_retry => {
                    let reason = self
                        .hubstaff_pause_reason
                        .clone()
                        .unwrap_or_else(|| "Hubstaff status could not be saved.".to_owned());
                    self.pause_for_hubstaff(&reason);
                }
                _ => {}
            }
        }
        let (now, mono) = self.now_and_mono();
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        if self.hubstaff_pause_retry
            || matches!(self.hubstaff_status, Some(HubstaffStatus::Stopped { .. }))
        {
            return;
        }
        let was_running = engine.running().is_some();
        let result = engine.tick(now, mono);
        let still_running = engine.running().is_some();
        self.notice = engine.notice().map(str::to_owned);
        if let Err(error) = result {
            self.set_error(error);
        }
        if was_running {
            // History shows the running timer's time, also while its
            // disclosure is collapsed (the wide layout always shows it).
            self.refresh_history();
        }
        if was_running && !still_running {
            self.refresh_events();
        }
    }

    /// Apply an edit to the Details form and save it. Past timers are
    /// read-only, so edits to them are ignored.
    fn edit_details(&mut self, edit: impl FnOnce(&mut Self)) {
        if !self.details_editable() {
            return;
        }
        edit(self);
        if self.save_details() {
            self.refresh_history();
        }
    }

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let tick = if self.engine.is_some() {
            time::every(TICK_INTERVAL).map(|_| Message::Tick)
        } else {
            Subscription::none()
        };
        Subscription::batch([
            tick,
            iced::window::close_requests().map(Message::CloseRequested),
        ])
    }
}

pub(crate) fn database_path_from_args() -> std::path::PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--db" {
            if let Some(path) = args.next() {
                return path.into();
            }
        }
    }
    default_database_path()
}
