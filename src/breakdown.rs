//! Derive the time breakdown of a linked session from its event log.
//!
//! Nothing here is stored: segments are recomputed from `events`, which stay
//! the single source of truth.

use crate::storage::{Event, EventKind};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;

/// One uninterrupted interval during which a single timer was running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub task_id: String,
    pub started_at: DateTime<Utc>,
    /// Active time, measured by the monotonic clock.
    pub duration_ms: i64,
    /// The event that closed the interval; `None` while it is still running.
    pub ended_by: Option<EventKind>,
}

impl Segment {
    /// Wall-clock end used for display.
    ///
    /// Placement is derived from the start plus the counted duration, not from
    /// the closing event's timestamp: a `recovery_pause` is written at the next
    /// launch, but counting stopped at the last checkpoint.
    pub fn ended_at(&self) -> DateTime<Utc> {
        self.started_at + Duration::milliseconds(self.duration_ms)
    }
}

/// Build segments from events of one or more tasks.
///
/// `live` supplies the current elapsed time of a timer that is still
/// running, so its open interval is included. Events must be in insertion
/// order per task (the storage layer orders by event ID). The result is
/// ordered by start time.
pub fn segments(events: &[Event], live: Option<(&str, i64)>) -> Vec<Segment> {
    let mut open: HashMap<&str, &Event> = HashMap::new();
    let mut result = Vec::new();
    for event in events {
        if event.kind.opens_interval() {
            open.insert(event.task_id.as_str(), event);
        } else if let Some(start) = open.remove(event.task_id.as_str()) {
            result.push((
                start.id,
                Segment {
                    task_id: event.task_id.clone(),
                    started_at: start.occurred_at,
                    duration_ms: (event.elapsed_ms - start.elapsed_ms).max(0),
                    ended_by: Some(event.kind),
                },
            ));
        }
    }
    if let Some((task_id, elapsed_ms)) = live {
        if let Some(start) = open.get(task_id) {
            result.push((
                start.id,
                Segment {
                    task_id: task_id.to_owned(),
                    started_at: start.occurred_at,
                    duration_ms: (elapsed_ms - start.elapsed_ms).max(0),
                    ended_by: None,
                },
            ));
        }
    }
    result.sort_by(|(a_id, a), (b_id, b)| a.started_at.cmp(&b.started_at).then(a_id.cmp(b_id)));
    result.into_iter().map(|(_, segment)| segment).collect()
}

/// Sum of segment durations per task.
pub fn totals_by_task(segments: &[Segment]) -> HashMap<String, i64> {
    let mut totals = HashMap::new();
    for segment in segments {
        *totals.entry(segment.task_id.clone()).or_insert(0) += segment.duration_ms;
    }
    totals
}

/// A row of the session timeline: either a segment or a stretch in which no
/// timer in the session was running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineEntry {
    Running(Segment),
    Idle {
        started_at: DateTime<Utc>,
        duration_ms: i64,
    },
}

/// Interleave idle gaps of at least `min_gap_ms` between segments.
pub fn timeline(segments: &[Segment], min_gap_ms: i64) -> Vec<TimelineEntry> {
    let mut entries = Vec::with_capacity(segments.len());
    let mut previous_end: Option<DateTime<Utc>> = None;
    for segment in segments {
        if let Some(end) = previous_end {
            let gap = (segment.started_at - end).num_milliseconds();
            if gap >= min_gap_ms {
                entries.push(TimelineEntry::Idle {
                    started_at: end,
                    duration_ms: gap,
                });
            }
        }
        let end = segment.ended_at();
        previous_end = Some(previous_end.map_or(end, |previous| previous.max(end)));
        entries.push(TimelineEntry::Running(segment.clone()));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(ms: i64) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(ms).single().unwrap()
    }

    fn event(id: i64, task: &str, kind: EventKind, at_ms: i64, elapsed_ms: i64) -> Event {
        Event {
            id,
            task_id: task.into(),
            kind,
            occurred_at: at(at_ms),
            elapsed_ms,
            details: None,
            cause: None,
            related_task_id: None,
        }
    }

    #[test]
    fn interleaved_switches_become_ordered_segments_and_idle_gaps() {
        let events = vec![
            event(1, "a", EventKind::Start, 0, 0),
            event(2, "a", EventKind::Pause, 5_000, 5_000),
            event(3, "b", EventKind::Start, 5_000, 0),
            event(4, "b", EventKind::Pause, 8_000, 3_000),
            event(5, "a", EventKind::Resume, 20_000, 5_000),
        ];
        let segments = segments(&events, Some(("a", 7_000)));
        assert_eq!(
            segments
                .iter()
                .map(|s| (s.task_id.as_str(), s.duration_ms, s.ended_by))
                .collect::<Vec<_>>(),
            vec![
                ("a", 5_000, Some(EventKind::Pause)),
                ("b", 3_000, Some(EventKind::Pause)),
                ("a", 2_000, None),
            ]
        );
        let totals = totals_by_task(&segments);
        assert_eq!(totals["a"], 7_000);
        assert_eq!(totals["b"], 3_000);

        let timeline = timeline(&segments, 1_000);
        assert_eq!(timeline.len(), 4);
        assert_eq!(
            timeline[2],
            TimelineEntry::Idle {
                started_at: at(8_000),
                duration_ms: 12_000
            }
        );
    }

    #[test]
    fn recovery_pause_is_placed_at_the_checkpoint_not_the_next_launch() {
        let events = vec![
            event(1, "a", EventKind::Start, 0, 0),
            event(2, "a", EventKind::RecoveryPause, 600_000, 3_000),
        ];
        let segments = segments(&events, None);
        assert_eq!(segments[0].duration_ms, 3_000);
        assert_eq!(segments[0].ended_at(), at(3_000));
    }

    #[test]
    fn stop_after_pause_adds_no_time() {
        let events = vec![
            event(1, "a", EventKind::Start, 0, 0),
            event(2, "a", EventKind::Pause, 4_000, 4_000),
            event(3, "a", EventKind::Stop, 9_000, 4_000),
        ];
        let segments = segments(&events, None);
        assert_eq!(segments.len(), 1);
        assert_eq!(totals_by_task(&segments)["a"], 4_000);
    }
}
