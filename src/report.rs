//! One selected session and its continuations, grouped into a single report.

use crate::storage::SessionSummary;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct ReportTimer {
    pub series_id: String,
    pub name: String,
    pub elapsed_ms: i64,
    /// Distinct nonempty notes, in recording order. Repeated carried notes
    /// appear once; changed notes remain available in the aggregate report.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SessionReport {
    pub timers: Vec<ReportTimer>,
    /// Saved portions and any current work, ordered by start time, for detail.
    pub recordings: Vec<SessionSummary>,
}

impl SessionReport {
    pub fn total_ms(&self) -> i64 {
        self.timers.iter().map(|timer| timer.elapsed_ms).sum()
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.recordings[0].session.started_at
    }

    pub fn last_activity(&self) -> DateTime<Utc> {
        self.recordings
            .iter()
            .map(SessionSummary::last_activity)
            .max()
            .unwrap_or_else(|| self.started_at())
    }

    pub fn contains_task(&self, task_id: &str) -> bool {
        self.recordings
            .iter()
            .any(|recording| recording.tasks.iter().any(|task| task.id == task_id))
    }
}

/// Follow both directions of continuation, including branches and sessions
/// that explicitly combine earlier work. Shared series retain the connection
/// even if an intermediate timer's continuation link was deleted.
/// Count each stored task once, then combine by stable series ID, never name.
pub fn aggregate(sessions: &[SessionSummary], selected: &str) -> Option<SessionReport> {
    let by_id: HashMap<_, _> = sessions
        .iter()
        .enumerate()
        .map(|(index, session)| (session.id(), index))
        .collect();
    let mut by_series: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, session) in sessions.iter().enumerate() {
        for task in &session.tasks {
            by_series.entry(&task.series_id).or_default().push(index);
        }
    }
    let mut queue = VecDeque::from([*by_id.get(selected)?]);
    let mut seen_sessions = HashSet::new();
    let mut seen_series = HashSet::new();
    while let Some(index) = queue.pop_front() {
        if !seen_sessions.insert(index) {
            continue;
        }
        let session = &sessions[index];
        for id in session
            .continues_sessions
            .iter()
            .chain(&session.continued_by_sessions)
        {
            if let Some(index) = by_id.get(id.as_str()) {
                queue.push_back(*index);
            }
        }
        for task in &session.tasks {
            if seen_series.insert(task.series_id.as_str()) {
                queue.extend(by_series[task.series_id.as_str()].iter().copied());
            }
        }
    }
    let mut recordings: Vec<_> = seen_sessions
        .into_iter()
        .map(|index| sessions[index].clone())
        .collect();
    recordings.sort_by(|a, b| {
        a.session
            .started_at
            .cmp(&b.session.started_at)
            .then_with(|| a.id().cmp(b.id()))
    });
    let mut timers: Vec<ReportTimer> = Vec::new();
    let mut timer_indices = HashMap::new();
    let mut seen_tasks = HashSet::new();
    let mut latest_names: HashMap<&str, DateTime<Utc>> = HashMap::new();
    for recording in &recordings {
        for task in &recording.tasks {
            if !seen_tasks.insert(task.id.as_str()) {
                continue;
            }
            let index = *timer_indices
                .entry(task.series_id.clone())
                .or_insert_with(|| {
                    timers.push(ReportTimer {
                        series_id: task.series_id.clone(),
                        name: task.label(),
                        elapsed_ms: 0,
                        notes: Vec::new(),
                    });
                    timers.len() - 1
                });
            let timer = &mut timers[index];
            timer.elapsed_ms += task.elapsed_ms.max(0);
            let latest = latest_names
                .entry(&task.series_id)
                .or_insert(task.updated_at);
            if task.updated_at >= *latest {
                timer.name = task.label();
                *latest = task.updated_at;
            }
            let note = &task.notes;
            if !note.trim().is_empty() && !timer.notes.iter().any(|existing| existing == note) {
                timer.notes.push(note.clone());
            }
        }
    }
    Some(SessionReport { timers, recordings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{SessionEnd, SessionRecord, Task, TaskState};
    use chrono::TimeZone;

    fn recording(id: &str, start: i64, tasks: &[(&str, &str, &str, &str, i64)]) -> SessionSummary {
        let at = Utc.timestamp_millis_opt(start).unwrap();
        SessionSummary {
            session: SessionRecord {
                id: id.into(),
                started_at: at,
                ended_at: Some(at),
                end_reason: Some(SessionEnd::User),
            },
            tasks: tasks
                .iter()
                .enumerate()
                .map(|(index, (task, series, name, notes, elapsed))| Task {
                    id: (*task).into(),
                    session_id: id.into(),
                    position: index as i64 + 1,
                    series_id: (*series).into(),
                    continues_task_id: None,
                    name: (*name).into(),
                    notes: (*notes).into(),
                    state: TaskState::Completed,
                    created_at: at,
                    updated_at: at,
                    completed_at: Some(at),
                    elapsed_ms: *elapsed,
                })
                .collect(),
            continues_sessions: Vec::new(),
            continued_by_sessions: Vec::new(),
        }
    }

    #[test]
    fn every_part_of_a_branched_continuation_produces_the_same_aggregate_and_full_notes() {
        let mut root = recording(
            "root",
            0,
            &[
                (
                    "a",
                    "review",
                    "Review",
                    "  First line\n    Second line  ",
                    6000,
                ),
                ("b", "email", "Email", "Mail notes", 4000),
            ],
        );
        let branch = recording(
            "branch",
            10000,
            &[
                (
                    "c",
                    "review",
                    "Review",
                    "  First line\n    Second line  ",
                    3000,
                ),
                ("d", "qa", "QA", "QA notes", 2000),
            ],
        );
        let other_branch = recording(
            "other-branch",
            20000,
            &[("e", "review", "Review", "Branch note", 4000)],
        );
        let leaf = recording(
            "leaf",
            30000,
            &[("f", "review", "Renamed Review", "Latest note", 2000)],
        );
        let unrelated = recording(
            "unrelated",
            40000,
            &[("g", "unrelated-work", "Review", "Unrelated note", 100000)],
        );
        // Repeated/cyclic links cannot duplicate a saved portion.
        root.continues_sessions = vec!["leaf".into()];
        let sessions = vec![unrelated, leaf, other_branch, branch, root];
        for selected in ["root", "branch", "other-branch", "leaf"] {
            let report = aggregate(&sessions, selected).unwrap();
            assert_eq!(report.recordings.len(), 4);
            assert_eq!(report.total_ms(), 21000);
            assert_eq!(report.timers.len(), 3);
            assert_eq!(report.timers[0].name, "Renamed Review");
            assert_eq!(report.timers[0].elapsed_ms, 15000);
            assert_eq!(
                report.timers[0].notes,
                [
                    "  First line\n    Second line  ",
                    "Branch note",
                    "Latest note"
                ]
            );
            assert!(!report.contains_task("g"));
        }
        assert!(aggregate(&sessions, "missing").is_none());
    }

    #[test]
    fn equal_names_are_separate_timers_and_explicitly_combined_work_joins_reports() {
        let mut combined = recording(
            "combined",
            100,
            &[
                ("a", "one", "Review", "One", 1000),
                ("b", "two", "Review", "Two", 2000),
            ],
        );
        let source = recording(
            "source",
            0,
            &[("c", "three", "Earlier work", "Earlier note", 3000)],
        );
        combined.continues_sessions = vec!["source".into()];
        let report = aggregate(&[combined, source], "combined").unwrap();
        assert_eq!(report.total_ms(), 6000);
        assert_eq!(report.timers.len(), 3);
        assert_eq!(
            report
                .timers
                .iter()
                .filter(|timer| timer.name == "Review")
                .count(),
            2
        );
    }
}
