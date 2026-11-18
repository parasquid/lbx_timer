use crate::breakdown::{self, Segment};
use crate::event_log::{self, Continuation, LogEntry};
use crate::storage::{
    timer_label, AppData, AppError, Event, EventKind, NewTimer, SeriesTotal, SessionEnd,
    SessionSummary, SwitchTarget, Task, TaskState,
};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use thiserror::Error;
use uuid::Uuid;

/// A forward wall-clock jump larger than this is treated as suspend/sleep.
pub const SLEEP_GAP_MS: i64 = 5_000;

/// Upper bound on timers in one session; it keeps the list readable.
pub const MAX_TIMERS: usize = 9;

#[derive(Debug, Error)]
pub enum TimerError {
    #[error(transparent)]
    Storage(#[from] AppError),
    #[error("there is no such timer in the current session")]
    UnknownTimer,
    #[error("a session can hold at most {MAX_TIMERS} timers")]
    TooManyTimers,
    #[error("only a timer that has never run can be removed")]
    TimerHasTime,
    #[error("a session always keeps at least one timer")]
    LastTimer,
    #[error("the current task cannot be deleted while it is open")]
    CannotDeleteActiveTask,
    #[error("the current session is already loaded")]
    CannotLoadCurrentSession,
    #[error("every timer of that session is already in the current session")]
    AlreadyLoaded,
    #[error("that session has no timers to load")]
    NothingToLoad,
}

/// What happens to a current session that already has recorded time when an
/// earlier session is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadMode {
    /// Add the loaded timers to the current session.
    AddToCurrent,
    /// End the current session first; the loaded timers start a new one.
    EndCurrentFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    /// Created but never started; nothing is stored yet.
    Unstarted,
    Running,
    Paused,
}

/// One member of the current session's linked timer set.
#[derive(Debug, Clone)]
pub struct Timer {
    /// Stable ID; becomes the task ID when the timer first starts.
    pub id: String,
    /// 1-based number used for the default label ("Timer 2").
    pub number: i64,
    pub name: String,
    pub notes: String,
    pub state: TimerState,
    /// Time in this session.
    pub elapsed_ms: i64,
    /// Hubstaff-confirmed exclusions. Raw local accounting is preserved in
    /// storage; visible time omits these automatically reconciled intervals.
    excluded_ms: i64,
    /// The piece of work this timer belongs to across sessions.
    pub series_id: String,
    /// The timer from an earlier session that this one continues.
    pub continues_task_id: Option<String>,
    /// Time and sessions recorded for this piece of work before this session.
    pub prior: SeriesTotal,
}

impl Timer {
    fn new(number: i64) -> Self {
        let id = Uuid::new_v4().to_string();
        Self {
            series_id: id.clone(),
            id,
            number,
            name: String::new(),
            notes: String::new(),
            state: TimerState::Unstarted,
            elapsed_ms: 0,
            excluded_ms: 0,
            continues_task_id: None,
            prior: SeriesTotal::default(),
        }
    }

    /// Whether this timer continues work from earlier sessions.
    pub fn is_continued(&self) -> bool {
        self.prior.sessions > 0
    }

    /// Time across every session of this piece of work, including this one.
    pub fn total_ms(&self) -> i64 {
        self.prior.total_ms + self.elapsed_ms.max(0)
    }

    /// Original locally counted time, before Hubstaff-backed exclusions.
    pub fn raw_elapsed_ms(&self) -> i64 {
        self.elapsed_ms + self.excluded_ms
    }

    /// Sessions with recorded time for this piece of work, counting this
    /// one once it has started.
    pub fn total_sessions(&self) -> i64 {
        self.prior.sessions + i64::from(self.is_persisted())
    }

    /// Untouched placeholder: never run, unnamed, no notes, not a
    /// continuation. Loading a session replaces such timers.
    fn is_pristine(&self) -> bool {
        self.state == TimerState::Unstarted
            && self.name.trim().is_empty()
            && self.notes.trim().is_empty()
            && self.continues_task_id.is_none()
    }

    /// Display name: the user's name or "Timer N".
    pub fn label(&self) -> String {
        timer_label(&self.name, self.number)
    }

    pub fn is_persisted(&self) -> bool {
        self.state != TimerState::Unstarted
    }
}

/// The linked set of timers for this launch. At most one timer runs; starting
/// any timer pauses the running one (like a radio group).
///
/// The session row is written lazily with the first start, so a launch with
/// no recorded time leaves nothing behind.
#[derive(Debug, Clone)]
pub struct CurrentSession {
    pub id: String,
    pub persisted: bool,
    pub timers: Vec<Timer>,
    next_number: i64,
}

impl CurrentSession {
    fn fresh() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            persisted: false,
            timers: vec![Timer::new(1)],
            next_number: 2,
        }
    }

    pub fn running(&self) -> Option<&Timer> {
        self.timers.iter().find(|t| t.state == TimerState::Running)
    }

    pub fn timer(&self, id: &str) -> Option<&Timer> {
        self.timers.iter().find(|t| t.id == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.timer(id).is_some()
    }

    /// Sum of every timer's elapsed time: the session total.
    pub fn total_ms(&self) -> i64 {
        self.timers.iter().map(|t| t.elapsed_ms.max(0)).sum()
    }

    pub fn has_recorded_time(&self) -> bool {
        self.timers.iter().any(Timer::is_persisted)
    }

    fn index_of(&self, id: &str) -> Result<usize, TimerError> {
        self.timers
            .iter()
            .position(|t| t.id == id)
            .ok_or(TimerError::UnknownTimer)
    }

    fn running_index(&self) -> Option<usize> {
        self.timers
            .iter()
            .position(|t| t.state == TimerState::Running)
    }
}

pub struct TimerEngine {
    db: AppData,
    session: CurrentSession,
    last_mono_ms: u64,
    last_wall_ms: i64,
    notice: Option<String>,
}

impl TimerEngine {
    /// Recover an uncertain running interval at the last checkpoint and close
    /// any session an unclean exit left open. Every launch then begins a new
    /// session holding one unstarted timer; nothing starts automatically.
    pub fn boot(mut db: AppData, now: DateTime<Utc>, mono_ms: u64) -> Result<Self, TimerError> {
        let mut notice = None;
        db.recover_time_verifications()?;
        // Close sessions first so their end time is the last checkpoint,
        // before the recovery event below touches `updated_at`.
        db.close_abandoned_sessions()?;
        if let Some(task) = db.active_task()? {
            if task.state == TaskState::Running {
                db.transition(
                    &task.id,
                    EventKind::RecoveryPause,
                    now,
                    task.elapsed_ms,
                    Some("Recovered after an unclean exit; time since the last checkpoint was not counted."),
                )?;
                notice = Some(
                    "Recovered the previous task. Time after its last checkpoint was not counted."
                        .to_owned(),
                );
            }
        }
        Ok(Self {
            db,
            session: CurrentSession::fresh(),
            last_mono_ms: mono_ms,
            last_wall_ms: now.timestamp_millis(),
            notice,
        })
    }

    pub fn session(&self) -> &CurrentSession {
        &self.session
    }

    pub fn timers(&self) -> &[Timer] {
        &self.session.timers
    }

    pub fn timer(&self, id: &str) -> Option<&Timer> {
        self.session.timer(id)
    }

    pub fn running(&self) -> Option<&Timer> {
        self.session.running()
    }

    /// All stored tasks, most recently updated first.
    pub fn history(&self) -> Result<Vec<Task>, TimerError> {
        Ok(self.db.tasks()?)
    }

    /// Stored history grouped by linked session.
    pub fn session_history(&self) -> Result<Vec<SessionSummary>, TimerError> {
        Ok(self.db.session_summaries()?)
    }

    /// Build a read-only aggregate snapshot, including current names, notes,
    /// and unstarted continuations that have not needed a database row yet.
    pub fn session_report(
        &self,
        selected: &str,
        as_of: DateTime<Utc>,
    ) -> Result<Option<crate::report::SessionReport>, TimerError> {
        let mut sessions = self.session_history()?;
        let current_index = sessions
            .iter()
            .position(|summary| summary.id() == self.session.id);
        let mut current = current_index
            .map(|index| sessions.remove(index))
            .unwrap_or_else(|| SessionSummary {
                session: crate::storage::SessionRecord {
                    id: self.session.id.clone(),
                    started_at: as_of,
                    ended_at: None,
                    end_reason: None,
                },
                tasks: Vec::new(),
                continues_sessions: Vec::new(),
                continued_by_sessions: Vec::new(),
            });
        for timer in &self.session.timers {
            if let Some(task) = current.tasks.iter_mut().find(|task| task.id == timer.id) {
                task.name = timer.name.clone();
                task.notes = timer.notes.clone();
                task.elapsed_ms = timer.elapsed_ms;
            } else {
                current.tasks.push(Task {
                    id: timer.id.clone(),
                    session_id: self.session.id.clone(),
                    position: timer.number,
                    series_id: timer.series_id.clone(),
                    continues_task_id: timer.continues_task_id.clone(),
                    name: timer.name.clone(),
                    notes: timer.notes.clone(),
                    state: if timer.state == TimerState::Running {
                        TaskState::Running
                    } else {
                        TaskState::Paused
                    },
                    created_at: as_of,
                    updated_at: as_of,
                    completed_at: None,
                    elapsed_ms: timer.elapsed_ms,
                });
            }
        }
        sessions.push(current);
        Ok(crate::report::aggregate(&sessions, selected))
    }

    /// Time segments of a stored session, including the live interval of
    /// the running timer when it is the current session.
    pub fn session_segments(&self, session_id: &str) -> Result<Vec<Segment>, TimerError> {
        let events = self.db.session_events(session_id)?;
        let live = (self.session.id == session_id)
            .then(|| self.session.running())
            .flatten()
            .map(|t| (t.id.as_str(), t.raw_elapsed_ms()));
        let segments = breakdown::segments(&events, live);
        Ok(crate::verification::reconciled_segments(
            segments,
            &self.db.time_verifications()?,
            &events,
        ))
    }

    pub fn events(&self, task_id: &str) -> Result<Vec<Event>, TimerError> {
        Ok(self.db.events(task_id)?)
    }

    pub fn task(&self, task_id: &str) -> Result<Option<Task>, TimerError> {
        Ok(self.db.task(task_id)?)
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    pub fn tick(&mut self, now: DateTime<Utc>, mono_ms: u64) -> Result<(), TimerError> {
        self.advance(now, mono_ms).map(|_| ())
    }

    pub fn time_verifications(
        &self,
    ) -> Result<Vec<crate::verification::TimeVerification>, TimerError> {
        Ok(self.db.time_verifications()?)
    }

    /// Mark the interval since the last verified running checkpoint. An
    /// existing open interval is retained through repeated failed responses.
    pub fn mark_time_unverified(
        &mut self,
        checkpoint: Option<&(String, DateTime<Utc>, i64)>,
        reason: &str,
        now: DateTime<Utc>,
        mono_ms: u64,
    ) -> Result<(), TimerError> {
        self.advance(now, mono_ms)?;
        let Some(timer) = self.running() else {
            return Ok(());
        };
        let (at, elapsed) = checkpoint
            .filter(|(id, _, elapsed)| *id == timer.id && *elapsed <= timer.raw_elapsed_ms())
            .map(|(_, at, elapsed)| (*at, *elapsed))
            .unwrap_or((now, timer.raw_elapsed_ms()));
        self.db
            .begin_time_verification(&timer.id.clone(), at, elapsed, reason)?;
        Ok(())
    }

    pub fn verified_checkpoint(
        &mut self,
        now: DateTime<Utc>,
        mono_ms: u64,
    ) -> Result<Option<(String, DateTime<Utc>, i64)>, TimerError> {
        self.advance(now, mono_ms)?;
        let Some(timer) = self.running() else {
            return Ok(None);
        };
        let checkpoint = (timer.id.clone(), now, timer.raw_elapsed_ms());
        self.db
            .close_time_verification(&checkpoint.0, now, checkpoint.2)?;
        Ok(Some(checkpoint))
    }

    pub fn save_verification_results(
        &mut self,
        results: &[crate::verification::VerificationResult],
    ) -> Result<(), TimerError> {
        self.db.save_verification_results(results)?;
        for timer in &mut self.session.timers {
            if timer.is_persisted() {
                if let Some(task) = self.db.task(&timer.id)? {
                    let raw = timer.raw_elapsed_ms();
                    timer.elapsed_ms = task.elapsed_ms;
                    timer.excluded_ms = raw - task.elapsed_ms;
                }
            }
        }
        self.refresh_priors()?;
        Ok(())
    }

    /// Append an unstarted timer. It does not affect the running timer.
    pub fn add_timer(&mut self) -> Result<String, TimerError> {
        if self.session.timers.len() >= MAX_TIMERS {
            return Err(TimerError::TooManyTimers);
        }
        let timer = Timer::new(self.session.next_number);
        self.session.next_number += 1;
        let id = timer.id.clone();
        self.session.timers.push(timer);
        Ok(id)
    }

    /// Remove a timer that has never run. Timers with recorded time stay in
    /// the session until it ends, so the session total never loses time.
    pub fn remove_timer(&mut self, id: &str) -> Result<(), TimerError> {
        let index = self.session.index_of(id)?;
        if self.session.timers[index].is_persisted() {
            return Err(TimerError::TimerHasTime);
        }
        if self.session.timers.len() == 1 {
            return Err(TimerError::LastTimer);
        }
        self.session.timers.remove(index);
        Ok(())
    }

    /// Start or resume timer `id`, pausing whichever timer was running.
    ///
    /// The pause and start are one transaction with one timestamp. Starting
    /// the timer that is already running does nothing.
    pub fn start(&mut self, id: &str, now: DateTime<Utc>, mono_ms: u64) -> Result<(), TimerError> {
        self.start_with_reason(id, now, mono_ms, "Triggered manually in LBX.")
    }

    /// Start or resume with a trigger retained in the event log. The same
    /// trigger is recorded on the previous timer's pause during a switch.
    pub fn start_with_reason(
        &mut self,
        id: &str,
        now: DateTime<Utc>,
        mono_ms: u64,
        reason: &str,
    ) -> Result<(), TimerError> {
        let index = self.session.index_of(id)?;
        if self.session.timers[index].state == TimerState::Running {
            return Ok(());
        }
        let slept = self.advance(now, mono_ms)?;
        let from_index = self.session.running_index();
        let from = from_index.map(|i| {
            let timer = &self.session.timers[i];
            (timer.id.as_str(), timer.raw_elapsed_ms())
        });
        let target = &self.session.timers[index];
        let to = match target.state {
            TimerState::Unstarted => SwitchTarget::Start(NewTimer {
                id: &target.id,
                name: &target.name,
                notes: &target.notes,
                position: target.number,
                series_id: &target.series_id,
                continues_task_id: target.continues_task_id.as_deref(),
            }),
            TimerState::Paused => SwitchTarget::Resume {
                task_id: &target.id,
                elapsed_ms: target.raw_elapsed_ms(),
            },
            TimerState::Running => return Ok(()),
        };
        self.db
            .switch_with_reason(&self.session.id, from, to, now, Some(reason))?;

        if let Some(i) = from_index {
            self.session.timers[i].state = TimerState::Paused;
        }
        self.session.timers[index].state = TimerState::Running;
        self.session.persisted = true;
        self.sync_clock(now, mono_ms);
        if !slept {
            self.notice = None;
        }
        Ok(())
    }

    /// Pause the running timer, if any. Every timer stays in the session.
    pub fn pause(&mut self, now: DateTime<Utc>, mono_ms: u64) -> Result<(), TimerError> {
        self.pause_with_reason(now, mono_ms, Some("Triggered manually in LBX."))
    }

    /// Pause the running timer and retain an automatic pause reason in the
    /// existing event details column.
    pub fn pause_with_reason(
        &mut self,
        now: DateTime<Utc>,
        mono_ms: u64,
        reason: Option<&str>,
    ) -> Result<(), TimerError> {
        self.advance(now, mono_ms)?;
        let Some(index) = self.session.running_index() else {
            return Ok(());
        };
        let timer = &self.session.timers[index];
        self.db.transition(
            &timer.id,
            EventKind::Pause,
            now,
            timer.raw_elapsed_ms(),
            reason,
        )?;
        self.session.timers[index].state = TimerState::Paused;
        self.sync_clock(now, mono_ms);
        Ok(())
    }

    /// Complete every timer, close the session, and begin a new session with
    /// one unstarted timer. Returns the ID of the stored session, if any time
    /// was recorded.
    pub fn end_session(
        &mut self,
        now: DateTime<Utc>,
        mono_ms: u64,
    ) -> Result<Option<String>, TimerError> {
        self.close_session(SessionEnd::User, now, mono_ms)
    }

    /// Pause the running timer for a window close and close the session.
    /// Timers stay paused in history; the next launch starts a new session.
    pub fn pause_for_close(&mut self, now: DateTime<Utc>, mono_ms: u64) -> Result<(), TimerError> {
        self.close_session(SessionEnd::AppClose, now, mono_ms)
            .map(|_| ())
    }

    /// Close the current session in storage (if anything was recorded) and
    /// replace it with a fresh one. Returns the closed session's ID.
    fn close_session(
        &mut self,
        reason: SessionEnd,
        now: DateTime<Utc>,
        mono_ms: u64,
    ) -> Result<Option<String>, TimerError> {
        self.advance(now, mono_ms)?;
        let closed = if self.session.persisted {
            let running = self
                .session
                .running()
                .map(|t| (t.id.as_str(), t.raw_elapsed_ms()));
            self.db
                .end_session(&self.session.id, now, reason, running)?;
            Some(self.session.id.clone())
        } else {
            None
        };
        self.session = CurrentSession::fresh();
        self.sync_clock(now, mono_ms);
        Ok(closed)
    }

    /// Bring the timers of an earlier session back as unstarted timers that
    /// continue it. Each loaded timer keeps the name and notes of the one it
    /// continues, links to it, and shows the time already recorded for that
    /// work. Untouched placeholder timers are replaced; timers whose work is
    /// already in the current session are skipped. Returns the new timer IDs.
    ///
    /// With [`LoadMode::EndCurrentFirst`], a current session that has recorded
    /// time is ended first, so the loaded timers start a new session.
    pub fn load_session(
        &mut self,
        session_id: &str,
        mode: LoadMode,
        now: DateTime<Utc>,
        mono_ms: u64,
    ) -> Result<Vec<String>, TimerError> {
        if session_id == self.session.id {
            return Err(TimerError::CannotLoadCurrentSession);
        }
        let source = self.db.session_tasks(session_id)?;
        if source.is_empty() {
            return Err(TimerError::NothingToLoad);
        }
        // Work out the resulting timer set before changing anything.
        let ending = mode == LoadMode::EndCurrentFirst && self.session.has_recorded_time();
        let kept: Vec<&Timer> = if ending {
            Vec::new()
        } else {
            self.session
                .timers
                .iter()
                .filter(|timer| !timer.is_pristine())
                .collect()
        };
        let present: HashSet<&str> = kept.iter().map(|timer| timer.series_id.as_str()).collect();
        let to_load: Vec<&Task> = source
            .iter()
            .filter(|task| !present.contains(task.series_id.as_str()))
            .collect();
        if to_load.is_empty() {
            return Err(TimerError::AlreadyLoaded);
        }
        if kept.len() + to_load.len() > MAX_TIMERS {
            return Err(TimerError::TooManyTimers);
        }
        let loaded: Vec<Task> = to_load.into_iter().cloned().collect();

        if ending {
            self.end_session(now, mono_ms)?;
        }
        self.session.timers.retain(|timer| !timer.is_pristine());
        if self.session.timers.is_empty() {
            self.session.next_number = 1;
        }
        let mut ids = Vec::with_capacity(loaded.len());
        for task in loaded {
            let mut timer = Timer::new(self.session.next_number);
            self.session.next_number += 1;
            // Carry the visible name, so an unnamed "Timer 2" stays "Timer 2".
            timer.name = task.label();
            timer.notes = task.notes;
            timer.series_id = task.series_id;
            timer.continues_task_id = Some(task.id);
            ids.push(timer.id.clone());
            self.session.timers.push(timer);
        }
        // Prior time is read after any ending above, so it includes the
        // time just recorded for the same work.
        self.refresh_priors()?;
        Ok(ids)
    }

    /// Total time of every piece of work in history, keyed by series.
    pub fn series_totals(
        &self,
    ) -> Result<std::collections::HashMap<String, SeriesTotal>, TimerError> {
        Ok(self.db.series_totals()?)
    }

    /// A readable, self-contained log of one timer's events: each line names
    /// the timer, the other timer involved in a switch, and the reason.
    pub fn event_log(&self, task_id: &str) -> Result<Vec<LogEntry>, TimerError> {
        let events = self.db.events(task_id)?;
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let name = self
            .label_of(task_id)?
            .unwrap_or_else(|| "Unknown timer".to_owned());
        let continues = match self
            .db
            .task(task_id)?
            .and_then(|task| task.continues_task_id)
        {
            Some(source_id) => self.continuation(&source_id)?,
            None => None,
        };
        // Other timers' names are looked up once per timer, not per event.
        let mut related_names: HashMap<String, Option<String>> = HashMap::new();
        let intervals = self.db.time_verifications()?;
        let mut entries = Vec::with_capacity(events.len());
        for event in &events {
            let related = match &event.related_task_id {
                Some(id) => match related_names.get(id) {
                    Some(name) => name.clone(),
                    None => {
                        let name = self.label_of(id)?;
                        related_names.insert(id.clone(), name.clone());
                        name
                    }
                },
                None => None,
            };
            let mut entry = event_log::entry(event, &name, related.as_deref(), continues.as_ref());
            entry.elapsed_ms = (entry.elapsed_ms
                - crate::verification::excluded_before(&intervals, task_id, event.elapsed_ms))
            .max(0);
            entries.push(entry);
        }
        for interval in intervals
            .iter()
            .filter(|interval| interval.task_id == task_id)
        {
            let end = interval
                .ended_at
                .map(|at| {
                    at.with_timezone(&chrono::Local)
                        .format("%b %d, %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|| "still running".into());
            entries.push(LogEntry {
                occurred_at: interval.ended_at.unwrap_or(interval.started_at),
                elapsed_ms: interval
                    .elapsed_end_ms
                    .map(|raw| {
                        (raw - crate::verification::excluded_before(&intervals, task_id, raw))
                            .max(0)
                    })
                    .unwrap_or_else(|| {
                        self.timer(task_id)
                            .map_or(interval.elapsed_start_ms, |timer| timer.elapsed_ms)
                    }),
                text: format!(
                    "{} time for “{}” from {} to {} — Hubstaff observation failed: {}. {}",
                    interval.state.label(),
                    name,
                    interval
                        .started_at
                        .with_timezone(&chrono::Local)
                        .format("%b %d, %H:%M:%S"),
                    end,
                    interval.reason,
                    if interval.evidence.is_empty() {
                        "Counted locally; awaiting Hubstaff audit evidence."
                    } else {
                        &interval.evidence
                    },
                ),
            });
        }
        entries.sort_by_key(|entry| entry.occurred_at);
        Ok(entries)
    }

    /// Current display name of a timer in this session or in history.
    fn label_of(&self, task_id: &str) -> Result<Option<String>, TimerError> {
        if let Some(timer) = self.session.timer(task_id) {
            return Ok(Some(timer.label()));
        }
        Ok(self.db.task(task_id)?.map(|task| task.label()))
    }

    /// The earlier timer a timer continues, for the event log.
    fn continuation(&self, source_id: &str) -> Result<Option<Continuation>, TimerError> {
        let Some(source) = self.db.task(source_id)? else {
            return Ok(None);
        };
        let session_started_at = self
            .db
            .session(&source.session_id)?
            .map(|session| session.started_at);
        Ok(Some(Continuation {
            name: source.label(),
            session_started_at,
        }))
    }

    /// Recompute each current timer's prior time, e.g. after history changed.
    fn refresh_priors(&mut self) -> Result<(), TimerError> {
        for index in 0..self.session.timers.len() {
            let timer = &self.session.timers[index];
            let total = self.db.series_total(&timer.series_id)?;
            let own = if timer.is_persisted() {
                self.db.task(&timer.id)?.map_or(0, |task| task.elapsed_ms)
            } else {
                0
            };
            let stored_here = i64::from(timer.is_persisted());
            self.session.timers[index].prior = SeriesTotal {
                total_ms: (total.total_ms - own).max(0),
                sessions: (total.sessions - stored_here).max(0),
            };
        }
        Ok(())
    }

    /// Update name and notes. Works for unstarted timers (kept in memory until their first start),
    /// stored timers of the current session, and history entries.
    pub fn update_name_notes(
        &mut self,
        task_id: &str,
        name: &str,
        notes: &str,
    ) -> Result<(), TimerError> {
        let stored = self.session.timer(task_id).is_none_or(Timer::is_persisted);
        if stored {
            self.db.update_name_notes(task_id, name, notes)?;
        }
        self.sync_timer_details(task_id, name, notes);
        Ok(())
    }

    fn sync_timer_details(&mut self, task_id: &str, name: &str, notes: &str) {
        if let Some(timer) = self.session.timers.iter_mut().find(|t| t.id == task_id) {
            timer.name = name.to_owned();
            timer.notes = notes.to_owned();
        }
    }

    /// Delete a saved task and its event timeline. Timers in the current
    /// session are protected.
    pub fn delete_history_task(&mut self, task_id: &str) -> Result<(), TimerError> {
        if self.session.contains(task_id) {
            return Err(TimerError::CannotDeleteActiveTask);
        }
        let Some(task) = self.db.task(task_id)? else {
            return Err(AppError::NotFound(format!("task {task_id}")).into());
        };
        if task.state == TaskState::Running {
            return Err(TimerError::CannotDeleteActiveTask);
        }
        self.db.delete_task(task_id)?;
        // The deleted timer may have held time for work continued here.
        self.refresh_priors()?;
        Ok(())
    }

    /// Accumulate monotonic time into the running timer and checkpoint it.
    /// Returns `true` when a sleep gap paused the running timer.
    fn advance(&mut self, now: DateTime<Utc>, mono_ms: u64) -> Result<bool, TimerError> {
        let Some(index) = self.session.running_index() else {
            self.sync_clock(now, mono_ms);
            return Ok(false);
        };
        let timer = &mut self.session.timers[index];

        let mono_delta = mono_ms.saturating_sub(self.last_mono_ms);
        let wall_delta = now.timestamp_millis().saturating_sub(self.last_wall_ms);
        let bounded_mono = mono_delta.min(i64::MAX as u64) as i64;
        let elapsed = timer.raw_elapsed_ms().saturating_add(bounded_mono);
        let sleep_detected = wall_delta > bounded_mono + SLEEP_GAP_MS;

        if sleep_detected {
            self.db.transition(
                &timer.id,
                EventKind::SleepPause,
                now,
                elapsed,
                Some("System suspend or a long clock gap was detected; review before resuming."),
            )?;
            timer.elapsed_ms = elapsed - timer.excluded_ms;
            timer.state = TimerState::Paused;
            self.notice = Some(
                "Paused after the computer slept or its clock jumped; that time was not counted."
                    .to_owned(),
            );
        } else if mono_delta > 0 {
            self.db.checkpoint(&timer.id, now, elapsed)?;
            timer.elapsed_ms = elapsed - timer.excluded_ms;
        }
        self.sync_clock(now, mono_ms);
        Ok(sleep_detected)
    }

    /// Anchor time measurement at this instant.
    fn sync_clock(&mut self, now: DateTime<Utc>, mono_ms: u64) {
        self.last_mono_ms = mono_ms;
        self.last_wall_ms = now.timestamp_millis();
    }
}

impl Drop for TimerEngine {
    /// Fallback when the engine is dropped without a window-close request:
    /// close the session at the last checkpoint, never counting the time since.
    fn drop(&mut self) {
        if self.session.persisted {
            let now = DateTime::from_timestamp_millis(self.last_wall_ms).unwrap_or_else(Utc::now);
            let mono = self.last_mono_ms;
            if let Err(error) = self.pause_for_close(now, mono) {
                eprintln!("lbx-timer: could not save close pause: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::EventCause;
    use chrono::TimeZone;
    use std::fs;

    fn test_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lbx-timer-{name}-{}", Uuid::new_v4()))
    }

    fn at(ms: i64) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(ms)
            .single()
            .expect("valid timestamp")
    }

    fn cleanup(path: &std::path::Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(format!("{}.lock", path.display()));
        let _ = fs::remove_file(format!("{}-wal", path.display()));
        let _ = fs::remove_file(format!("{}-shm", path.display()));
    }

    fn kinds(engine: &TimerEngine, id: &str) -> Vec<EventKind> {
        engine
            .events(id)
            .unwrap()
            .iter()
            .map(|event| event.kind)
            .collect()
    }

    fn first_timer(engine: &TimerEngine) -> String {
        engine.timers()[0].id.clone()
    }

    #[test]
    fn pause_resume_excludes_paused_time() {
        let path = test_path("pause-resume");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let id = first_timer(&engine);
        engine.start(&id, at(0), 0).unwrap();
        engine.tick(at(5_000), 5_000).unwrap();
        engine.pause(at(5_000), 5_000).unwrap();
        engine.start(&id, at(15_000), 15_000).unwrap();
        engine.tick(at(17_000), 17_000).unwrap();
        engine.end_session(at(17_000), 17_000).unwrap();
        let task = engine.task(&id).unwrap().unwrap();
        assert_eq!(task.elapsed_ms, 7_000);
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(
            kinds(&engine, &id),
            vec![
                EventKind::Start,
                EventKind::Pause,
                EventKind::Resume,
                EventKind::Stop
            ]
        );
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn sleep_gap_pauses_without_counting_the_gap() {
        let path = test_path("sleep");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let id = first_timer(&engine);
        engine.start(&id, at(0), 0).unwrap();
        engine.tick(at(2_000), 2_000).unwrap();
        engine.tick(at(60_000), 3_000).unwrap();
        let timer = engine.timer(&id).unwrap();
        assert_eq!(timer.state, TimerState::Paused);
        assert_eq!(timer.elapsed_ms, 3_000);
        assert!(engine.running().is_none());
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn every_boot_starts_a_new_session_with_one_unstarted_timer() {
        let path = test_path("manual-start");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        assert_eq!(engine.timers().len(), 1);
        assert_eq!(engine.timers()[0].state, TimerState::Unstarted);
        assert!(engine.history().unwrap().is_empty());
        let first_session = engine.session().id.clone();

        let id = first_timer(&engine);
        engine.start(&id, at(0), 0).unwrap();
        engine.pause_for_close(at(5_000), 5_000).unwrap();
        drop(engine);

        let reopened = TimerEngine::boot(AppData::open(&path).unwrap(), at(60_000), 0).unwrap();
        assert_ne!(reopened.session().id, first_session);
        assert_eq!(reopened.timers().len(), 1);
        assert_eq!(reopened.timers()[0].state, TimerState::Unstarted);
        assert_ne!(reopened.timers()[0].id, id);
        let previous = reopened.task(&id).unwrap().unwrap();
        assert_eq!(previous.state, TaskState::Paused);
        assert_eq!(previous.elapsed_ms, 5_000);
        assert_eq!(
            kinds(&reopened, &id),
            vec![EventKind::Start, EventKind::ClosePause]
        );
        let history = reopened.session_history().unwrap();
        assert_eq!(history.len(), 1);
        let session = &history[0].session;
        assert_eq!(session.id, first_session);
        assert_eq!(session.end_reason, Some(SessionEnd::AppClose));
        assert_eq!(session.ended_at, Some(at(5_000)));
        drop(reopened);
        cleanup(&path);
    }

    #[test]
    fn running_task_recovery_does_not_count_the_crash_gap() {
        let path = test_path("recovery");
        let id;
        {
            let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
            id = first_timer(&engine);
            engine.start(&id, at(0), 0).unwrap();
            engine.tick(at(3_000), 3_000).unwrap();
            // Simulate a crash: skip the close-on-drop so nothing more is
            // written, then release the database lock.
            engine.session.persisted = false;
            drop(engine);
        }
        let engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(20_000), 20_000).unwrap();
        assert!(engine.running().is_none());
        let recovered = engine.task(&id).unwrap().unwrap();
        assert_eq!(recovered.elapsed_ms, 3_000);
        assert_eq!(recovered.state, TaskState::Paused);
        assert_eq!(
            kinds(&engine, &id),
            vec![EventKind::Start, EventKind::RecoveryPause]
        );
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn starting_a_timer_pauses_the_running_one_like_a_radio_group() {
        let path = test_path("radio");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let a = first_timer(&engine);
        let b = engine.add_timer().unwrap();
        assert_eq!(engine.timer(&b).unwrap().state, TimerState::Unstarted);

        engine.start(&a, at(0), 0).unwrap();
        engine.tick(at(5_000), 5_000).unwrap();
        engine.start(&b, at(5_000), 5_000).unwrap();
        assert_eq!(engine.timer(&a).unwrap().state, TimerState::Paused);
        assert_eq!(engine.running().unwrap().id, b);
        engine.tick(at(8_000), 8_000).unwrap();
        engine.start(&a, at(8_000), 8_000).unwrap();
        engine.tick(at(10_000), 10_000).unwrap();
        assert_eq!(engine.session().total_ms(), 10_000);

        let session_id = engine.end_session(at(10_000), 10_000).unwrap().unwrap();
        let a_task = engine.task(&a).unwrap().unwrap();
        let b_task = engine.task(&b).unwrap().unwrap();
        assert_eq!((a_task.elapsed_ms, b_task.elapsed_ms), (7_000, 3_000));
        assert_eq!(a_task.state, TaskState::Completed);
        assert_eq!(b_task.state, TaskState::Completed);

        let a_events = engine.events(&a).unwrap();
        assert_eq!(
            a_events
                .iter()
                .map(|e| (e.kind, e.cause))
                .collect::<Vec<_>>(),
            vec![
                (EventKind::Start, None),
                (EventKind::Pause, Some(EventCause::Switch)),
                (EventKind::Resume, Some(EventCause::Switch)),
                (EventKind::Stop, Some(EventCause::EndSession)),
            ]
        );
        let b_events = engine.events(&b).unwrap();
        assert_eq!(b_events[0].cause, Some(EventCause::Switch));
        // The handover shares one timestamp: no gap and no overlap.
        assert_eq!(a_events[1].occurred_at, b_events[0].occurred_at);
        assert_eq!(b_events[1].occurred_at, a_events[2].occurred_at);

        let history = engine.session_history().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].total_ms(), 10_000);
        assert_eq!(
            history[0]
                .tasks
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>(),
            vec![a.clone(), b.clone()]
        );
        let segments = engine.session_segments(&session_id).unwrap();
        assert_eq!(
            segments
                .iter()
                .map(|s| (s.task_id.clone(), s.duration_ms))
                .collect::<Vec<_>>(),
            vec![(a.clone(), 5_000), (b.clone(), 3_000), (a, 2_000)]
        );

        // A fresh session with one unstarted timer follows.
        assert_ne!(engine.session().id, session_id);
        assert_eq!(engine.timers().len(), 1);
        assert_eq!(engine.timers()[0].state, TimerState::Unstarted);
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn adding_and_removing_timers_never_touches_the_running_timer() {
        let path = test_path("add-remove");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let a = first_timer(&engine);
        assert!(matches!(
            engine.remove_timer(&a),
            Err(TimerError::LastTimer)
        ));
        engine.start(&a, at(0), 0).unwrap();
        let b = engine.add_timer().unwrap();
        assert_eq!(engine.running().unwrap().id, a);
        assert_eq!(engine.timer(&b).unwrap().label(), "Timer 2");
        assert!(matches!(
            engine.remove_timer(&a),
            Err(TimerError::TimerHasTime)
        ));
        engine.remove_timer(&b).unwrap();
        assert_eq!(engine.timers().len(), 1);
        assert_eq!(engine.running().unwrap().id, a);
        for _ in 1..MAX_TIMERS {
            engine.add_timer().unwrap();
        }
        assert!(matches!(engine.add_timer(), Err(TimerError::TooManyTimers)));
        // Numbers are not reused within a session.
        assert_eq!(engine.timers()[1].number, 3);
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn unstarted_timers_keep_details_in_memory_until_first_start() {
        let path = test_path("drafts");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let a = first_timer(&engine);
        engine.update_name_notes(&a, "Review", "queue 7").unwrap();
        assert!(engine.history().unwrap().is_empty());
        assert_eq!(engine.timer(&a).unwrap().label(), "Review");
        engine.start(&a, at(0), 0).unwrap();
        let task = engine.task(&a).unwrap().unwrap();
        assert_eq!(
            (task.name.as_str(), task.notes.as_str()),
            ("Review", "queue 7")
        );
        assert_eq!(task.position, 1);
        assert_eq!(
            Some(task.session_id.as_str()),
            Some(engine.session().id.as_str())
        );
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn starting_the_running_timer_again_records_nothing() {
        let path = test_path("double-click");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let a = first_timer(&engine);
        engine.start(&a, at(0), 0).unwrap();
        engine.start(&a, at(1_000), 1_000).unwrap();
        assert_eq!(kinds(&engine, &a), vec![EventKind::Start]);
        drop(engine);
        cleanup(&path);
    }

    /// Randomized switch sequences: the sum of segments must equal each
    /// timer's total, and the timer totals must add up to the session total.
    #[test]
    fn breakdown_always_reconciles_with_totals() {
        for seed in 1..=60u64 {
            let path = test_path(&format!("reconcile-{seed}"));
            let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
            let mut state = seed;
            let mut next = move |bound: u64| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 33) % bound
            };
            let mut clock = 0i64;
            for _ in 0..40 {
                clock += 1 + next(4_000) as i64;
                let (now, mono) = (at(clock), clock as u64);
                match next(6) {
                    0 if engine.timers().len() < MAX_TIMERS => {
                        engine.add_timer().unwrap();
                    }
                    1 => engine.pause(now, mono).unwrap(),
                    2 => engine.tick(now, mono).unwrap(),
                    _ => {
                        let pick = next(engine.timers().len() as u64) as usize;
                        let id = engine.timers()[pick].id.clone();
                        engine.start(&id, now, mono).unwrap();
                    }
                }
            }
            // Bring the running timer up to date before reading totals.
            engine.tick(at(clock), clock as u64).unwrap();
            let session_total = engine.session().total_ms();
            let was_running = engine.running().is_some();
            let Some(session_id) = engine
                .end_session(at(clock + 1_000), (clock + 1_000) as u64)
                .unwrap()
            else {
                drop(engine);
                cleanup(&path);
                continue;
            };
            let segments = engine.session_segments(&session_id).unwrap();
            let totals = breakdown::totals_by_task(&segments);
            let history = engine.session_history().unwrap();
            let summary = history.iter().find(|s| s.id() == session_id).unwrap();
            for task in &summary.tasks {
                assert_eq!(
                    totals.get(&task.id).copied().unwrap_or(0),
                    task.elapsed_ms,
                    "seed {seed}: segments disagree with task total"
                );
            }
            assert_eq!(
                summary.total_ms(),
                session_total + 1_000 * i64::from(was_running),
                "seed {seed}"
            );
            assert_eq!(
                segments.iter().map(|s| s.duration_ms).sum::<i64>(),
                summary.total_ms(),
                "seed {seed}"
            );
            drop(engine);
            cleanup(&path);
        }
    }

    /// Record a finished session: "Review" 5 s, "Email" 3 s. Returns
    /// (session id, review id, email id).
    fn finished_session(engine: &mut TimerEngine, start: i64) -> (String, String, String) {
        let review = first_timer(engine);
        let email = engine.add_timer().unwrap();
        engine
            .update_name_notes(&review, "Review", "queue 7")
            .unwrap();
        engine.start(&review, at(start), start as u64).unwrap();
        engine
            .tick(at(start + 5_000), (start + 5_000) as u64)
            .unwrap();
        engine
            .start(&email, at(start + 5_000), (start + 5_000) as u64)
            .unwrap();
        engine
            .tick(at(start + 8_000), (start + 8_000) as u64)
            .unwrap();
        let session = engine
            .end_session(at(start + 8_000), (start + 8_000) as u64)
            .unwrap()
            .unwrap();
        (session, review, email)
    }

    #[test]
    fn loading_a_session_continues_its_timers_with_running_totals() {
        let path = test_path("load");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let (first, review, email) = finished_session(&mut engine, 0);

        let ids = engine
            .load_session(&first, LoadMode::AddToCurrent, at(100_000), 100_000)
            .unwrap();
        // The untouched placeholder was replaced by the two loaded timers.
        assert_eq!(engine.timers().len(), 2);
        let loaded = engine.timer(&ids[0]).unwrap().clone();
        assert_eq!(loaded.label(), "Review");
        assert_eq!(loaded.notes, "queue 7");
        assert_eq!(loaded.number, 1);
        assert_eq!(loaded.continues_task_id.as_deref(), Some(review.as_str()));
        assert_eq!(
            loaded.prior,
            SeriesTotal {
                total_ms: 5_000,
                sessions: 1
            }
        );
        assert!(loaded.is_continued());
        // An unnamed original keeps its visible name.
        assert_eq!(engine.timer(&ids[1]).unwrap().label(), "Timer 2");
        assert!(engine.history().unwrap().len() == 2, "loading stored rows");

        engine.start(&ids[0], at(100_000), 100_000).unwrap();
        engine.tick(at(102_000), 102_000).unwrap();
        let running = engine.timer(&ids[0]).unwrap();
        assert_eq!(running.elapsed_ms, 2_000);
        assert_eq!(running.total_ms(), 7_000);
        assert_eq!(running.total_sessions(), 2);
        let second = engine.end_session(at(102_000), 102_000).unwrap().unwrap();

        let stored = engine.task(&ids[0]).unwrap().unwrap();
        let original = engine.task(&review).unwrap().unwrap();
        assert_eq!(stored.series_id, original.series_id);
        assert_eq!(stored.continues_task_id.as_deref(), Some(review.as_str()));
        let totals = engine.series_totals().unwrap();
        assert_eq!(
            totals[&original.series_id],
            SeriesTotal {
                total_ms: 7_000,
                sessions: 2
            }
        );
        // Email was loaded but never run, so nothing was stored for it.
        assert_eq!(
            totals[&engine.task(&email).unwrap().unwrap().series_id].sessions,
            1
        );

        let history = engine.session_history().unwrap();
        let newer = history.iter().find(|s| s.id() == second).unwrap();
        let older = history.iter().find(|s| s.id() == first).unwrap();
        assert_eq!(newer.continues_sessions, vec![first.clone()]);
        assert_eq!(older.continued_by_sessions, vec![second.clone()]);
        assert!(older.continues_sessions.is_empty());
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn loading_respects_the_current_session_and_rejects_without_changes() {
        let path = test_path("load-modes");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let (first, _, _) = finished_session(&mut engine, 0);

        // The current session already has time: add to it...
        let own = first_timer(&engine);
        engine.start(&own, at(50_000), 50_000).unwrap();
        let current = engine.session().id.clone();
        let added = engine
            .load_session(&first, LoadMode::AddToCurrent, at(51_000), 51_000)
            .unwrap();
        assert_eq!(engine.session().id, current);
        assert_eq!(engine.timers().len(), 3);
        assert_eq!(
            engine.running().unwrap().id,
            own,
            "loading touched the running timer"
        );
        assert!(added.iter().all(|id| engine.timer(id).unwrap().number > 1));

        // ...loading it again adds nothing...
        assert!(matches!(
            engine.load_session(&first, LoadMode::AddToCurrent, at(52_000), 52_000),
            Err(TimerError::AlreadyLoaded)
        ));
        assert!(matches!(
            engine.load_session(&current, LoadMode::AddToCurrent, at(52_000), 52_000),
            Err(TimerError::CannotLoadCurrentSession)
        ));

        // ...or end the current session first.
        let ids = engine
            .load_session(&first, LoadMode::EndCurrentFirst, at(53_000), 53_000)
            .unwrap();
        assert_ne!(engine.session().id, current);
        assert_eq!(engine.timers().len(), 2);
        assert!(engine.running().is_none());
        assert_eq!(engine.timers()[0].id, ids[0]);
        let ended = engine.session_history().unwrap();
        assert!(ended
            .iter()
            .any(|s| s.id() == current && s.session.ended_at.is_some()));

        // Too many timers: rejected before anything changes. Named timers
        // are not placeholders, so none of them would be replaced.
        for n in 0..(MAX_TIMERS - 2) {
            let id = engine.add_timer().unwrap();
            engine
                .update_name_notes(&id, &format!("Extra {n}"), "")
                .unwrap();
        }
        assert_eq!(engine.timers().len(), MAX_TIMERS);
        let before: Vec<String> = engine.timers().iter().map(|t| t.id.clone()).collect();
        let result = engine.load_session(&current, LoadMode::AddToCurrent, at(54_000), 54_000);
        assert!(
            matches!(result, Err(TimerError::TooManyTimers)),
            "{result:?}"
        );
        let after: Vec<String> = engine.timers().iter().map(|t| t.id.clone()).collect();
        assert_eq!(before, after);
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn deleting_the_continued_timer_keeps_the_continuation_working() {
        let path = test_path("load-delete");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let (first, review, _) = finished_session(&mut engine, 0);
        let ids = engine
            .load_session(&first, LoadMode::AddToCurrent, at(20_000), 20_000)
            .unwrap();
        engine.delete_history_task(&review).unwrap();
        let loaded = engine.timer(&ids[0]).unwrap();
        assert_eq!(loaded.prior, SeriesTotal::default(), "stale prior time");
        engine.start(&ids[0], at(20_000), 20_000).unwrap();
        let stored = engine.task(&ids[0]).unwrap().unwrap();
        assert_eq!(
            stored.continues_task_id, None,
            "link to a deleted timer kept"
        );
        drop(engine);
        cleanup(&path);
    }

    #[test]
    fn the_event_log_names_both_timers_of_a_switch() {
        let path = test_path("log");
        let mut engine = TimerEngine::boot(AppData::open(&path).unwrap(), at(0), 0).unwrap();
        let (first, review, email) = finished_session(&mut engine, 0);
        let log = |engine: &TimerEngine, id: &str| -> Vec<String> {
            engine
                .event_log(id)
                .unwrap()
                .into_iter()
                .map(|e| e.text)
                .collect()
        };
        assert_eq!(
            log(&engine, &review),
            [
                "Started “Review” — Triggered manually in LBX.",
                "Paused “Review” — switched to “Timer 2”; Triggered manually in LBX.",
                "Stopped “Review” — the session was ended manually in LBX",
            ]
        );
        assert_eq!(
            log(&engine, &email),
            [
                "Started “Timer 2” — switched from “Review”; Triggered manually in LBX.",
                "Stopped “Timer 2” — the session was ended manually in LBX",
            ]
        );
        let ids = engine
            .load_session(&first, LoadMode::AddToCurrent, at(20_000), 20_000)
            .unwrap();
        engine.start(&ids[0], at(20_000), 20_000).unwrap();
        let line = &log(&engine, &ids[0])[0];
        assert!(
            line.starts_with("Started “Review” — continues “Review” from the "),
            "{line}"
        );
        assert!(
            engine.event_log(&ids[1]).unwrap().is_empty(),
            "unstarted timer has events"
        );
        drop(engine);
        cleanup(&path);
    }
}
