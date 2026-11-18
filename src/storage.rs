use crate::verification::{TimeVerification, VerificationResult, VerificationState};
use chrono::{DateTime, SecondsFormat, Utc};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use std::collections::{BTreeSet, HashMap};
use std::fs::{create_dir_all, File, OpenOptions};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The current database layout. Supported earlier releases (starting at 5)
/// migrate transactionally; unknown future/experimental layouts are refused.
pub const SCHEMA_VERSION: i64 = 6;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database is already in use: {0}")]
    AlreadyRunning(PathBuf),
    /// A value read from the database is not one this build writes.
    #[error("invalid stored value: {0}")]
    Corrupt(String),
    #[error("invalid stored timestamp: {0}")]
    InvalidTimestamp(String),
    /// The requested change does not fit the current state, e.g. pausing a
    /// timer that is not running.
    #[error("{0}")]
    InvalidTransition(String),
    #[error("no such {0}")]
    NotFound(String),
    #[error(
        "unsupported database schema version {found} (this build uses version {expected}). \
         The file was not changed; move it aside to start fresh."
    )]
    UnsupportedSchema {
        path: PathBuf,
        found: i64,
        expected: i64,
    },
}

/// Define a fieldless enum stored as text: each variant's string is written
/// once and drives `as_str`, parsing, and the schema's `CHECK` constraint.
macro_rules! stored_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident => $text:literal,)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            fn parse(value: &str) -> Result<Self, AppError> {
                match value {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(AppError::Corrupt(format!(
                        "{} {value:?}",
                        stringify!($name)
                    ))),
                }
            }

            /// `'a', 'b', …` for an SQL `IN (…)` list.
            fn sql_values() -> String {
                Self::ALL
                    .iter()
                    .map(|value| format!("'{}'", value.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }
    };
}

stored_enum! {
    pub enum TaskState {
        Running => "running",
        Paused => "paused",
        Completed => "completed",
    }
}

stored_enum! {
    pub enum EventKind {
        Start => "start",
        Pause => "pause",
        Resume => "resume",
        Stop => "stop",
        SleepPause => "sleep_pause",
        RecoveryPause => "recovery_pause",
        ClosePause => "close_pause",
    }
}

impl EventKind {
    /// Whether this event begins an interval in which the timer counts time.
    pub fn opens_interval(self) -> bool {
        matches!(self, Self::Start | Self::Resume)
    }
}

stored_enum! {
    /// Why an event happened when it was not a direct action on that timer.
    /// Switching uses the ordinary `pause`/`resume`/`start` kinds; the cause
    /// records that the transition was part of a handover or of End session.
    pub enum EventCause {
        /// Another timer in the same session was started.
        Switch => "switch",
        /// The user ended the session, completing every timer in it.
        EndSession => "end_session",
    }
}

stored_enum! {
    pub enum SessionEnd {
        /// The user pressed End session.
        User => "user",
        /// The window closed; the running timer was paused.
        AppClose => "app_close",
        /// Found still open at launch after an unclean exit.
        Recovered => "recovered",
    }
}

/// A stored timer: one member of a session's linked set.
#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub session_id: String,
    /// 1-based timer number within its session ("Timer 2").
    pub position: i64,
    /// Identifies one piece of work across sessions: the ID of the first
    /// timer in a chain of continuations (its own ID when not continued).
    pub series_id: String,
    /// The timer in an earlier session that this timer continues, if any.
    pub continues_task_id: Option<String>,
    pub name: String,
    pub notes: String,
    pub state: TaskState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub elapsed_ms: i64,
}

impl Task {
    /// Display name: the user's name, or "Timer N".
    pub fn label(&self) -> String {
        timer_label(&self.name, self.position)
    }
}

/// The name shown for a timer: its own name, or "Timer N" while unnamed.
pub fn timer_label(name: &str, number: i64) -> String {
    if name.trim().is_empty() {
        format!("Timer {number}")
    } else {
        name.to_owned()
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    pub id: i64,
    pub task_id: String,
    pub kind: EventKind,
    pub occurred_at: DateTime<Utc>,
    pub elapsed_ms: i64,
    pub details: Option<String>,
    pub cause: Option<EventCause>,
    /// The other timer involved: the one switched to (on a pause) or switched
    /// from (on a start or resume). `None` after that timer was deleted.
    pub related_task_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub end_reason: Option<SessionEnd>,
}

/// One history entry: a linked session and its timers.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session: SessionRecord,
    /// Ordered by timer number.
    pub tasks: Vec<Task>,
    /// Earlier sessions whose timers this session continues.
    pub continues_sessions: Vec<String>,
    /// Later sessions that continue timers of this session.
    pub continued_by_sessions: Vec<String>,
}

impl SessionSummary {
    pub fn id(&self) -> &str {
        &self.session.id
    }

    pub fn total_ms(&self) -> i64 {
        self.tasks.iter().map(|task| task.elapsed_ms.max(0)).sum()
    }

    pub fn last_activity(&self) -> DateTime<Utc> {
        self.tasks
            .iter()
            .map(|task| task.updated_at)
            .max()
            .unwrap_or(self.session.started_at)
    }
}

/// Accumulated time of one piece of work across the sessions it spans.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeriesTotal {
    pub total_ms: i64,
    /// Stored timers in the series; each belongs to a different session.
    pub sessions: i64,
}

/// A timer that has never run yet and is being started for the first time.
#[derive(Debug, Clone, Copy)]
pub struct NewTimer<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub notes: &'a str,
    pub position: i64,
    pub series_id: &'a str,
    pub continues_task_id: Option<&'a str>,
}

/// The timer that becomes the running member of a session.
#[derive(Debug, Clone, Copy)]
pub enum SwitchTarget<'a> {
    /// Resume a paused timer at its accumulated elapsed time.
    Resume { task_id: &'a str, elapsed_ms: i64 },
    /// Create and start a timer that has not run before.
    Start(NewTimer<'a>),
}

const RAW_TASK_COLUMNS: &str = "id, session_id, position, name, notes, state, created_at,
        updated_at, completed_at, elapsed_ms, series_id, continues_task_id";
const TASK_COLUMNS: &str = "id, session_id, position, name, notes, state, created_at,
        updated_at, completed_at,
        MAX(0, elapsed_ms - COALESCE((SELECT SUM(excluded_ms) FROM time_verifications WHERE task_id = tasks.id), 0)),
        series_id, continues_task_id";

fn timestamp(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_timestamp(value: String) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(&value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| AppError::InvalidTimestamp(value))
}

fn task_not_found(task_id: &str) -> AppError {
    AppError::NotFound(format!("task {task_id}"))
}

fn parent_dir(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

/// Why an event happened, recorded alongside it.
#[derive(Debug, Clone, Copy, Default)]
struct Context<'a> {
    details: Option<&'a str>,
    cause: Option<EventCause>,
    related_task_id: Option<&'a str>,
}

/// Apply one state transition and its event inside an open transaction.
fn apply_transition(
    tx: &Transaction<'_>,
    task_id: &str,
    kind: EventKind,
    at: DateTime<Utc>,
    elapsed_ms: i64,
    context: Context<'_>,
) -> Result<(), AppError> {
    let Context {
        details,
        cause,
        related_task_id,
    } = context;
    let when = timestamp(&at);
    let current: String = tx.query_row(
        "SELECT state FROM tasks WHERE id = ?1",
        params![task_id],
        |row| row.get(0),
    )?;
    let current = TaskState::parse(&current)?;
    let next = match kind {
        EventKind::Pause
        | EventKind::SleepPause
        | EventKind::RecoveryPause
        | EventKind::ClosePause => {
            if current != TaskState::Running {
                return Err(AppError::InvalidTransition(format!(
                    "cannot {} a {} task",
                    kind.as_str(),
                    current.as_str()
                )));
            }
            TaskState::Paused
        }
        EventKind::Resume => {
            if current != TaskState::Paused {
                return Err(AppError::InvalidTransition(format!(
                    "cannot resume a {} task",
                    current.as_str()
                )));
            }
            TaskState::Running
        }
        EventKind::Stop => {
            if current == TaskState::Completed {
                return Err(AppError::InvalidTransition(
                    "the task is already completed".into(),
                ));
            }
            TaskState::Completed
        }
        EventKind::Start => {
            return Err(AppError::InvalidTransition(
                "start is only valid for a new task".into(),
            ));
        }
    };
    tx.execute(
        "UPDATE tasks
            SET state = ?2,
                updated_at = ?3,
                completed_at = CASE WHEN ?2 = 'completed' THEN ?3 ELSE completed_at END,
                elapsed_ms = ?4
          WHERE id = ?1",
        params![task_id, next.as_str(), when, elapsed_ms.max(0)],
    )?;
    tx.execute(
        "INSERT INTO events
            (task_id, kind, occurred_at, elapsed_ms, details, cause, related_task_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            task_id,
            kind.as_str(),
            when,
            elapsed_ms.max(0),
            details,
            cause.map(EventCause::as_str),
            related_task_id
        ],
    )?;
    if !kind.opens_interval() {
        tx.execute(
            "UPDATE time_verifications SET ended_at = ?2, elapsed_end_ms = ?3
             WHERE task_id = ?1 AND ended_at IS NULL",
            params![task_id, when, elapsed_ms.max(0)],
        )?;
    }
    Ok(())
}

/// Insert a new running task and its start event inside an open transaction.
fn insert_started(
    tx: &Transaction<'_>,
    timer: NewTimer<'_>,
    session_id: &str,
    at: DateTime<Utc>,
    cause: Option<EventCause>,
    related_task_id: Option<&str>,
    details: Option<&str>,
) -> Result<(), AppError> {
    let when = timestamp(&at);
    // The timer being continued may have been deleted from history since the
    // continuation was loaded; the link is then dropped rather than failing.
    tx.execute(
        "INSERT INTO tasks
            (id, session_id, position, name, notes, state, created_at, updated_at,
             elapsed_ms, series_id, continues_task_id)
         VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?6, 0, ?7,
                 (SELECT id FROM tasks WHERE id = ?8))",
        params![
            timer.id,
            session_id,
            timer.position,
            timer.name,
            timer.notes,
            when,
            timer.series_id,
            timer.continues_task_id
        ],
    )?;
    tx.execute(
        "INSERT INTO events (task_id, kind, occurred_at, elapsed_ms, cause, related_task_id, details)
         VALUES (?1, 'start', ?2, 0, ?3, ?4, ?5)",
        params![
            timer.id,
            when,
            cause.map(EventCause::as_str),
            related_task_id,
            details
        ],
    )?;
    Ok(())
}

/// Open the SQLite database and hold an exclusive lock for this process.
pub struct AppData {
    conn: Connection,
    _lock: File,
}

impl AppData {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AppError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = parent_dir(&path) {
            create_dir_all(parent)?;
        }

        let lock_path = PathBuf::from(format!("{}.lock", path.display()));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        if let Err(error) = lock.try_lock_exclusive() {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(AppError::AlreadyRunning(path));
            }
            return Err(error.into());
        }

        // Inspect an existing file read-only first: a database this build
        // does not own must be refused without being written to (even
        // switching the journal mode or checkpointing a WAL rewrites it).
        let is_empty = check_schema(&path)?;

        let mut conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;",
        )?;
        if is_empty {
            create_schema(&mut conn)?;
        }
        migrate_verifications(&mut conn)?;

        Ok(Self { conn, _lock: lock })
    }

    pub fn begin_time_verification(
        &mut self,
        task_id: &str,
        started_at: DateTime<Utc>,
        elapsed_start_ms: i64,
        reason: &str,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO time_verifications (task_id, started_at, elapsed_start_ms, reason)
             SELECT ?1, ?2, ?3, ?4 WHERE NOT EXISTS
             (SELECT 1 FROM time_verifications WHERE task_id = ?1 AND ended_at IS NULL)",
            params![task_id, timestamp(&started_at), elapsed_start_ms, reason],
        )?;
        Ok(())
    }

    pub fn close_time_verification(
        &mut self,
        task_id: &str,
        at: DateTime<Utc>,
        elapsed_ms: i64,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "UPDATE time_verifications SET ended_at = ?2, elapsed_end_ms = ?3
             WHERE task_id = ?1 AND ended_at IS NULL",
            params![task_id, timestamp(&at), elapsed_ms],
        )?;
        Ok(())
    }

    pub fn recover_time_verifications(&mut self) -> Result<(), AppError> {
        self.conn.execute_batch(
            "UPDATE time_verifications SET
                ended_at = (SELECT updated_at FROM tasks WHERE id = task_id),
                elapsed_end_ms = (SELECT elapsed_ms FROM tasks WHERE id = task_id)
             WHERE ended_at IS NULL;",
        )?;
        Ok(())
    }

    pub fn time_verifications(&self) -> Result<Vec<TimeVerification>, AppError> {
        let mut statement = self.conn.prepare(
            "SELECT id, task_id, started_at, ended_at, elapsed_start_ms, elapsed_end_ms, state, reason, evidence, excluded_ranges
             FROM time_verifications ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            let state: String = row.get(6)?;
            let state = match state.as_str() {
                "unverified" => VerificationState::Unverified,
                "confirmed" => VerificationState::Confirmed,
                "adjusted" => VerificationState::Adjusted,
                _ => return Err(conversion_error(6, AppError::Corrupt(state))),
            };
            let ranges: String = row.get(9)?;
            let ranges: Vec<[String; 2]> = serde_json::from_str(&ranges)
                .map_err(|e| conversion_error(9, AppError::Corrupt(e.to_string())))?;
            let excluded_ranges = ranges
                .into_iter()
                .map(|[start, end]| Ok((parse_timestamp(start)?, parse_timestamp(end)?)))
                .collect::<Result<_, AppError>>()
                .map_err(|e| conversion_error(9, e))?;
            Ok(TimeVerification {
                id: row.get(0)?,
                task_id: row.get(1)?,
                started_at: parse_timestamp(row.get(2)?).map_err(|e| conversion_error(2, e))?,
                ended_at: row
                    .get::<_, Option<String>>(3)?
                    .map(parse_timestamp)
                    .transpose()
                    .map_err(|e| conversion_error(3, e))?,
                elapsed_start_ms: row.get(4)?,
                elapsed_end_ms: row.get(5)?,
                state,
                reason: row.get(7)?,
                evidence: row.get(8)?,
                excluded_ranges,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn save_verification_results(
        &mut self,
        results: &[VerificationResult],
    ) -> Result<(), AppError> {
        let tx = self.conn.transaction()?;
        for result in results {
            let boundary: Option<(String, i64, i64)> = tx
                .query_row(
                    "SELECT started_at, elapsed_start_ms, elapsed_end_ms FROM time_verifications
                 WHERE id = ?1 AND ended_at = ?2 AND state = 'unverified'",
                    params![result.id, timestamp(&result.ended_at)],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((started_at, elapsed_start, elapsed_end)) = boundary else {
                continue;
            };
            let started_at = parse_timestamp(started_at)?;
            let mut previous_end = started_at;
            for &(start, end) in &result.excluded_ranges {
                if start < previous_end
                    || end <= start
                    || end > result.ended_at
                    || result.state != VerificationState::Adjusted
                {
                    return Err(AppError::InvalidTransition(
                        "invalid Hubstaff reconciliation boundaries".into(),
                    ));
                }
                previous_end = end;
            }
            let excluded_ms: i64 = crate::verification::excluded_elapsed_ranges(
                started_at,
                result.ended_at,
                elapsed_start,
                elapsed_end,
                &result.excluded_ranges,
            )
            .iter()
            .map(|(start, end)| end - start)
            .sum();
            let encoded: Vec<_> = result
                .excluded_ranges
                .iter()
                .map(|(start, end)| [timestamp(start), timestamp(end)])
                .collect();
            let encoded =
                serde_json::to_string(&encoded).map_err(|e| AppError::Corrupt(e.to_string()))?;
            tx.execute(
                "UPDATE time_verifications SET state = ?3, evidence = ?4, excluded_ranges = ?5, excluded_ms = ?6
                 WHERE id = ?1 AND ended_at = ?2 AND state = 'unverified'",
                params![
                    result.id,
                    timestamp(&result.ended_at),
                    result.state.as_str(),
                    result.evidence,
                    encoded,
                    excluded_ms
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn transition(
        &mut self,
        task_id: &str,
        kind: EventKind,
        at: DateTime<Utc>,
        elapsed_ms: i64,
        details: Option<&str>,
    ) -> Result<Task, AppError> {
        let tx = self.conn.transaction()?;
        let context = Context {
            details,
            ..Context::default()
        };
        apply_transition(&tx, task_id, kind, at, elapsed_ms, context)?;
        tx.commit()?;
        self.task(task_id)?.ok_or_else(|| task_not_found(task_id))
    }

    /// Make `to` the running timer of `session_id`, pausing `from` first.
    ///
    /// Everything happens in one transaction with one timestamp: the session
    /// row is created on its first start, the previous timer is paused, and
    /// the target is started or resumed. A failure leaves every row as it was,
    /// and the one-running-task index is never violated in between.
    pub fn switch(
        &mut self,
        session_id: &str,
        from: Option<(&str, i64)>,
        to: SwitchTarget<'_>,
        at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        self.switch_with_reason(session_id, from, to, at, None)
    }

    /// Switch atomically, retaining the trigger on both affected events.
    pub fn switch_with_reason(
        &mut self,
        session_id: &str,
        from: Option<(&str, i64)>,
        to: SwitchTarget<'_>,
        at: DateTime<Utc>,
        details: Option<&str>,
    ) -> Result<(), AppError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO sessions (id, started_at) VALUES (?1, ?2)",
            params![session_id, timestamp(&at)],
        )?;
        let ended_at: Option<String> = tx
            .query_row(
                "SELECT ended_at FROM sessions WHERE id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if ended_at.is_some() {
            return Err(AppError::InvalidTransition(format!(
                "session {session_id} has already ended"
            )));
        }
        let cause = from.map(|_| EventCause::Switch);
        let from_id = from.map(|(id, _)| id);
        let to_id = match to {
            SwitchTarget::Resume { task_id, .. } => task_id,
            SwitchTarget::Start(timer) => timer.id,
        };
        if let Some((from_id, elapsed_ms)) = from {
            apply_transition(
                &tx,
                from_id,
                EventKind::Pause,
                at,
                elapsed_ms,
                Context {
                    cause: Some(EventCause::Switch),
                    related_task_id: Some(to_id),
                    details,
                },
            )?;
        }
        match to {
            SwitchTarget::Resume {
                task_id,
                elapsed_ms,
            } => {
                apply_transition(
                    &tx,
                    task_id,
                    EventKind::Resume,
                    at,
                    elapsed_ms,
                    Context {
                        cause,
                        related_task_id: from_id,
                        details,
                    },
                )?;
            }
            SwitchTarget::Start(timer) => {
                insert_started(&tx, timer, session_id, at, cause, from_id, details)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// End a linked session in one transaction.
    ///
    /// `running` carries the live elapsed time of the running timer, if any.
    /// [`SessionEnd::User`] completes every unfinished timer in the session
    /// with a `stop` event. [`SessionEnd::AppClose`] only pauses the running
    /// timer (`close_pause`) so the record matches what happened.
    pub fn end_session(
        &mut self,
        session_id: &str,
        at: DateTime<Utc>,
        reason: SessionEnd,
        running: Option<(&str, i64)>,
    ) -> Result<(), AppError> {
        let tx = self.conn.transaction()?;
        match reason {
            SessionEnd::Recovered => {
                return Err(AppError::InvalidTransition(
                    "abandoned sessions are closed by close_abandoned_sessions".into(),
                ));
            }
            SessionEnd::User => {
                let open: Vec<(String, i64)> = {
                    let mut statement = tx.prepare(
                        "SELECT id, elapsed_ms FROM tasks
                          WHERE session_id = ?1 AND state <> 'completed'
                          ORDER BY position, created_at",
                    )?;
                    let rows = statement
                        .query_map(params![session_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
                    rows.collect::<Result<_, _>>()?
                };
                for (task_id, stored_elapsed) in open {
                    let elapsed = running
                        .filter(|(id, _)| *id == task_id)
                        .map_or(stored_elapsed, |(_, live)| live);
                    apply_transition(
                        &tx,
                        &task_id,
                        EventKind::Stop,
                        at,
                        elapsed,
                        Context {
                            cause: Some(EventCause::EndSession),
                            ..Context::default()
                        },
                    )?;
                }
            }
            SessionEnd::AppClose => {
                if let Some((task_id, elapsed)) = running {
                    apply_transition(
                        &tx,
                        task_id,
                        EventKind::ClosePause,
                        at,
                        elapsed,
                        Context {
                            details: Some("Window closed; session saved paused in history."),
                            ..Context::default()
                        },
                    )?;
                }
            }
        }
        let changed = tx.execute(
            "UPDATE sessions SET ended_at = ?2, end_reason = ?3
              WHERE id = ?1 AND ended_at IS NULL",
            params![session_id, timestamp(&at), reason.as_str()],
        )?;
        if changed != 1 {
            return Err(AppError::InvalidTransition(format!(
                "session {session_id} is not open"
            )));
        }
        tx.commit()?;
        Ok(())
    }

    /// Close sessions left open by an unclean exit. The end time is the last
    /// saved checkpoint of any of their timers, not the time of this launch.
    /// Returns the number of sessions closed.
    pub fn close_abandoned_sessions(&mut self) -> Result<usize, AppError> {
        let changed = self.conn.execute(
            "UPDATE sessions
                SET ended_at = COALESCE(
                        (SELECT MAX(updated_at) FROM tasks WHERE tasks.session_id = sessions.id),
                        started_at),
                    end_reason = 'recovered'
              WHERE ended_at IS NULL",
            [],
        )?;
        Ok(changed)
    }

    pub fn checkpoint(
        &mut self,
        task_id: &str,
        at: DateTime<Utc>,
        elapsed_ms: i64,
    ) -> Result<(), AppError> {
        let changed = self.conn.execute(
            "UPDATE tasks SET elapsed_ms = ?2, updated_at = ?3
              WHERE id = ?1 AND state = 'running'",
            params![task_id, elapsed_ms.max(0), timestamp(&at)],
        )?;
        if changed != 1 {
            return Err(AppError::InvalidTransition(format!(
                "cannot checkpoint task {task_id}: it is not running"
            )));
        }
        Ok(())
    }

    pub fn update_name_notes(
        &mut self,
        task_id: &str,
        name: &str,
        notes: &str,
    ) -> Result<Task, AppError> {
        let changed = self.conn.execute(
            "UPDATE tasks SET name = ?2, notes = ?3
              WHERE id = ?1",
            params![task_id, name, notes],
        )?;
        if changed != 1 {
            return Err(task_not_found(task_id));
        }
        self.task(task_id)?.ok_or_else(|| task_not_found(task_id))
    }

    /// Delete a saved task and its event timeline in one transaction. A
    /// session left without timers is removed with it.
    pub fn delete_task(&mut self, task_id: &str) -> Result<(), AppError> {
        let tx = self.conn.transaction()?;
        let session_id: Option<String> = tx
            .query_row(
                "SELECT session_id FROM tasks WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()?;
        let changed = tx.execute(
            "DELETE FROM tasks WHERE id = ?1 AND state <> 'running'",
            params![task_id],
        )?;
        let Some(session_id) = session_id else {
            return Err(task_not_found(task_id));
        };
        if changed != 1 {
            return Err(AppError::InvalidTransition(
                "a running task cannot be deleted".into(),
            ));
        }
        tx.execute(
            "DELETE FROM sessions
              WHERE id = ?1
                AND ended_at IS NOT NULL
                AND NOT EXISTS (SELECT 1 FROM tasks WHERE session_id = ?1)",
            params![session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn task(&self, task_id: &str) -> Result<Option<Task>, AppError> {
        self.conn
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
                params![task_id],
                row_task,
            )
            .optional()
            .map_err(AppError::from)
    }

    pub fn active_task(&self) -> Result<Option<Task>, AppError> {
        // Recovery writes a checkpoint event and retains original local time.
        // Display/history queries instead use reconciled TASK_COLUMNS.
        self.conn
            .query_row(
                &format!(
                    "SELECT {RAW_TASK_COLUMNS} FROM tasks WHERE state = 'running'
                      ORDER BY updated_at DESC LIMIT 1"
                ),
                [],
                row_task,
            )
            .optional()
            .map_err(AppError::from)
    }

    pub fn tasks(&self) -> Result<Vec<Task>, AppError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks ORDER BY updated_at DESC"
        ))?;
        let rows = statement.query_map([], row_task)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn session(&self, session_id: &str) -> Result<Option<SessionRecord>, AppError> {
        self.conn
            .query_row(
                "SELECT id, started_at, ended_at, end_reason FROM sessions WHERE id = ?1",
                params![session_id],
                row_session,
            )
            .optional()
            .map_err(AppError::from)
    }

    pub fn sessions(&self) -> Result<Vec<SessionRecord>, AppError> {
        let mut statement = self.conn.prepare(
            "SELECT id, started_at, ended_at, end_reason FROM sessions ORDER BY started_at DESC",
        )?;
        let rows = statement.query_map([], row_session)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// History grouped by linked session, most recently active first.
    /// Sessions without stored timers are omitted.
    pub fn session_summaries(&self) -> Result<Vec<SessionSummary>, AppError> {
        let tasks = self.tasks()?;
        let session_of: HashMap<&str, &str> = tasks
            .iter()
            .map(|task| (task.id.as_str(), task.session_id.as_str()))
            .collect();
        // Session links in both directions, from continued timers.
        let mut continues: HashMap<&str, BTreeSet<&str>> = HashMap::new();
        let mut continued_by: HashMap<&str, BTreeSet<&str>> = HashMap::new();
        for task in &tasks {
            let Some(source) = task
                .continues_task_id
                .as_deref()
                .and_then(|id| session_of.get(id).copied())
            else {
                continue;
            };
            if source != task.session_id {
                continues
                    .entry(&task.session_id)
                    .or_default()
                    .insert(source);
                continued_by
                    .entry(source)
                    .or_default()
                    .insert(&task.session_id);
            }
        }
        let ids = |map: &HashMap<&str, BTreeSet<&str>>, id: &str| -> Vec<String> {
            map.get(id)
                .map(|set| set.iter().map(|id| (*id).to_owned()).collect())
                .unwrap_or_default()
        };

        let mut grouped: HashMap<String, Vec<Task>> = HashMap::new();
        for task in tasks.iter().cloned() {
            grouped
                .entry(task.session_id.clone())
                .or_default()
                .push(task);
        }
        let mut summaries: Vec<SessionSummary> = self
            .sessions()?
            .into_iter()
            .filter_map(|session| {
                let mut tasks = grouped.remove(&session.id)?;
                tasks.sort_by_key(|task| task.position);
                let continues_sessions = ids(&continues, &session.id);
                let continued_by_sessions = ids(&continued_by, &session.id);
                Some(SessionSummary {
                    session,
                    tasks,
                    continues_sessions,
                    continued_by_sessions,
                })
            })
            .collect();
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.last_activity()));
        Ok(summaries)
    }

    /// Stored timers of one session, ordered by timer number.
    pub fn session_tasks(&self, session_id: &str) -> Result<Vec<Task>, AppError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE session_id = ?1 ORDER BY position"
        ))?;
        let rows = statement.query_map(params![session_id], row_task)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Total time and number of stored timers in each series.
    pub fn series_totals(&self) -> Result<HashMap<String, SeriesTotal>, AppError> {
        let mut statement = self
            .conn
            .prepare("SELECT series_id, SUM(MAX(0, elapsed_ms - COALESCE((SELECT SUM(excluded_ms) FROM time_verifications WHERE task_id = tasks.id), 0))), COUNT(*) FROM tasks GROUP BY series_id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                SeriesTotal {
                    total_ms: row.get(1)?,
                    sessions: row.get(2)?,
                },
            ))
        })?;
        rows.collect::<Result<_, _>>().map_err(AppError::from)
    }

    /// Total for one series; zero for a series with no stored timers.
    pub fn series_total(&self, series_id: &str) -> Result<SeriesTotal, AppError> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(MAX(0, elapsed_ms - COALESCE((SELECT SUM(excluded_ms) FROM time_verifications WHERE task_id = tasks.id), 0))), 0), COUNT(*) FROM tasks WHERE series_id = ?1",
                params![series_id],
                |row| {
                    Ok(SeriesTotal {
                        total_ms: row.get(0)?,
                        sessions: row.get(1)?,
                    })
                },
            )
            .map_err(AppError::from)
    }

    pub fn events(&self, task_id: &str) -> Result<Vec<Event>, AppError> {
        self.query_events(
            "SELECT id, task_id, kind, occurred_at, elapsed_ms, details, cause, related_task_id
               FROM events WHERE task_id = ?1 ORDER BY id",
            task_id,
        )
    }

    /// Every event of every timer in a session, in insertion order.
    pub fn session_events(&self, session_id: &str) -> Result<Vec<Event>, AppError> {
        self.query_events(
            "SELECT e.id, e.task_id, e.kind, e.occurred_at, e.elapsed_ms, e.details, e.cause,
                    e.related_task_id
               FROM events e JOIN tasks t ON t.id = e.task_id
              WHERE t.session_id = ?1 ORDER BY e.id",
            session_id,
        )
    }

    fn query_events(&self, sql: &str, key: &str) -> Result<Vec<Event>, AppError> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map(params![key], |row| {
            let kind: String = row.get(2)?;
            let occurred_at: String = row.get(3)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                kind,
                occurred_at,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (id, task_id, kind, occurred_at, elapsed_ms, details, cause, related_task_id) =
                row?;
            Ok(Event {
                id,
                task_id,
                kind: EventKind::parse(&kind)?,
                occurred_at: parse_timestamp(occurred_at)?,
                elapsed_ms,
                details,
                cause: cause.as_deref().map(EventCause::parse).transpose()?,
                related_task_id,
            })
        })
        .collect()
    }
}

/// Returns `true` for a missing or empty database (the schema must be
/// created) and `false` for a database at [`SCHEMA_VERSION`].
fn check_schema(path: &Path) -> Result<bool, AppError> {
    if std::fs::metadata(path).map_or(true, |meta| meta.len() == 0) {
        return Ok(true);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let objects: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    match (version, objects) {
        (5 | SCHEMA_VERSION, _) => Ok(false),
        (0, 0) => Ok(true),
        _ => Err(AppError::UnsupportedSchema {
            path: path.to_path_buf(),
            found: version,
            expected: SCHEMA_VERSION,
        }),
    }
}

fn create_schema(conn: &mut Connection) -> Result<(), AppError> {
    let tx = conn.transaction()?;
    tx.execute_batch(&format!(
        "CREATE TABLE sessions (
             id TEXT PRIMARY KEY NOT NULL,
             started_at TEXT NOT NULL,
             ended_at TEXT,
             end_reason TEXT CHECK (end_reason IN ({end_reasons})),
             CHECK ((ended_at IS NULL) = (end_reason IS NULL))
         );
         -- At most one session is open: the one belonging to the running app.
         CREATE UNIQUE INDEX one_open_session ON sessions((1)) WHERE ended_at IS NULL;

         CREATE TABLE tasks (
             id TEXT PRIMARY KEY NOT NULL,
             session_id TEXT NOT NULL REFERENCES sessions(id),
             position INTEGER NOT NULL CHECK (position >= 1),
             name TEXT NOT NULL DEFAULT '',
             notes TEXT NOT NULL DEFAULT '',
             state TEXT NOT NULL CHECK (state IN ({states})),
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             completed_at TEXT,
             elapsed_ms INTEGER NOT NULL DEFAULT 0 CHECK (elapsed_ms >= 0),
             series_id TEXT NOT NULL,
             continues_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
             UNIQUE (session_id, position)
         );
         CREATE INDEX tasks_series ON tasks(series_id);
         -- The radio-group rule: at most one timer runs, across all sessions.
         CREATE UNIQUE INDEX one_running_task ON tasks((1)) WHERE state = 'running';
         CREATE INDEX tasks_updated_at ON tasks(updated_at DESC);

         CREATE TABLE events (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
             kind TEXT NOT NULL CHECK (kind IN ({kinds})),
             cause TEXT CHECK (cause IN ({causes})),
             -- Deferred: a switch pauses the old timer (naming the new one)
             -- before the new timer's row is inserted in the same transaction.
             related_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL
                 DEFERRABLE INITIALLY DEFERRED,
             occurred_at TEXT NOT NULL,
             elapsed_ms INTEGER NOT NULL CHECK (elapsed_ms >= 0),
             details TEXT
         );
         CREATE INDEX events_task_id ON events(task_id, id);

         PRAGMA user_version = 5;",
        end_reasons = SessionEnd::sql_values(),
        states = TaskState::sql_values(),
        kinds = EventKind::sql_values(),
        causes = EventCause::sql_values(),
    ))?;
    tx.commit()?;
    Ok(())
}

/// Version 5's timer and event rows remain byte-for-byte equivalent. Only
/// verification metadata is added, atomically, before version 6 is announced.
fn migrate_verifications(conn: &mut Connection) -> Result<(), AppError> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE time_verifications (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
             started_at TEXT NOT NULL,
             ended_at TEXT,
             elapsed_start_ms INTEGER NOT NULL CHECK (elapsed_start_ms >= 0),
             elapsed_end_ms INTEGER CHECK (elapsed_end_ms >= elapsed_start_ms),
             state TEXT NOT NULL DEFAULT 'unverified' CHECK (state IN ('unverified', 'confirmed', 'adjusted')),
             reason TEXT NOT NULL,
             evidence TEXT NOT NULL DEFAULT '',
             excluded_ranges TEXT NOT NULL DEFAULT '[]',
             excluded_ms INTEGER NOT NULL DEFAULT 0 CHECK (excluded_ms >= 0),
             CHECK ((ended_at IS NULL) = (elapsed_end_ms IS NULL)),
             CHECK (state = 'unverified' OR ended_at IS NOT NULL)
         );
         CREATE UNIQUE INDEX one_open_verification ON time_verifications(task_id) WHERE ended_at IS NULL;
         CREATE INDEX verification_task ON time_verifications(task_id);
         PRAGMA user_version = 6;",
    )?;
    tx.commit()?;
    Ok(())
}

fn conversion_error(index: usize, error: AppError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error))
}

fn row_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let created_at: String = row.get(6)?;
    let updated_at: String = row.get(7)?;
    let completed_at: Option<String> = row.get(8)?;
    Ok(Task {
        id: row.get(0)?,
        session_id: row.get(1)?,
        position: row.get(2)?,
        name: row.get(3)?,
        notes: row.get(4)?,
        state: TaskState::parse(&row.get::<_, String>(5)?)
            .map_err(|error| conversion_error(5, error))?,
        created_at: parse_timestamp(created_at).map_err(|error| conversion_error(6, error))?,
        updated_at: parse_timestamp(updated_at).map_err(|error| conversion_error(7, error))?,
        completed_at: completed_at
            .map(parse_timestamp)
            .transpose()
            .map_err(|error| conversion_error(8, error))?,
        elapsed_ms: row.get(9)?,
        series_id: row.get(10)?,
        continues_task_id: row.get(11)?,
    })
}

fn row_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let started_at: String = row.get(1)?;
    let ended_at: Option<String> = row.get(2)?;
    let end_reason: Option<String> = row.get(3)?;
    Ok(SessionRecord {
        id: row.get(0)?,
        started_at: parse_timestamp(started_at).map_err(|error| conversion_error(1, error))?,
        ended_at: ended_at
            .map(parse_timestamp)
            .transpose()
            .map_err(|error| conversion_error(2, error))?,
        end_reason: end_reason
            .as_deref()
            .map(SessionEnd::parse)
            .transpose()
            .map_err(|error| conversion_error(3, error))?,
    })
}

pub fn default_database_path() -> PathBuf {
    if let Some(path) = std::env::var_os("LBX_TIMER_DB") {
        return PathBuf::from(path);
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join("lbx-timer/timer.sqlite3");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/lbx-timer/timer.sqlite3");
    }
    PathBuf::from("lbx-timer.sqlite3")
}
