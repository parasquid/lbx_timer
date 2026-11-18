//! Durable verification of time counted while Hubstaff could not be observed.

use chrono::{DateTime, Utc};
use std::io::Read;
use std::path::{Path, PathBuf};

pub type TimeRange = (DateTime<Utc>, DateTime<Utc>);
pub type AuditAssessment = (VerificationState, String, Vec<TimeRange>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationState {
    Unverified,
    Confirmed,
    Adjusted,
}

impl VerificationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Confirmed => "confirmed",
            Self::Adjusted => "adjusted",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unverified => "Unverified",
            Self::Confirmed => "Confirmed",
            Self::Adjusted => "Adjusted",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimeVerification {
    pub id: i64,
    pub task_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub elapsed_start_ms: i64,
    pub elapsed_end_ms: Option<i64>,
    pub state: VerificationState,
    pub reason: String,
    pub evidence: String,
    pub excluded_ranges: Vec<TimeRange>,
}

#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub id: i64,
    pub ended_at: DateTime<Utc>,
    pub state: VerificationState,
    pub evidence: String,
    pub excluded_ranges: Vec<TimeRange>,
}

/// Map audit wall time into the locally counted interval. This shared mapping
/// keeps deductions, event totals, and the timeline equal despite rounding.
pub fn excluded_elapsed_ranges(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    elapsed_start: i64,
    elapsed_end: i64,
    ranges: &[TimeRange],
) -> Vec<(i64, i64)> {
    let wall_ms = (end - start).num_milliseconds();
    let counted = elapsed_end - elapsed_start;
    if wall_ms <= 0 || counted <= 0 {
        return Vec::new();
    }
    let map = |at: DateTime<Utc>| {
        elapsed_start
            + (((at - start).num_milliseconds() as i128 * counted as i128) / wall_ms as i128) as i64
    };
    ranges
        .iter()
        .map(|(from, to)| (map(*from), map(*to)))
        .collect()
}

impl TimeVerification {
    pub fn excluded_elapsed_ranges(&self) -> Vec<(i64, i64)> {
        match (self.ended_at, self.elapsed_end_ms) {
            (Some(end), Some(elapsed)) => excluded_elapsed_ranges(
                self.started_at,
                end,
                self.elapsed_start_ms,
                elapsed,
                &self.excluded_ranges,
            ),
            _ => Vec::new(),
        }
    }
}

pub fn audit_log_path() -> Option<PathBuf> {
    std::env::var_os("LBX_HUBSTAFF_AUDIT_LOG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
                })
                .map(|data| data.join("Hubstaff/logs/audit.log"))
        })
}

/// Read only the local audit log, off the GUI thread. Missing, truncated, or
/// inconsistent evidence leaves time unverified rather than assuming support.
pub async fn check_audit(
    path: Option<PathBuf>,
    intervals: Vec<TimeVerification>,
    observed_at: DateTime<Utc>,
    tracking: bool,
) -> Vec<VerificationResult> {
    let fallback = intervals.clone();
    tokio::task::spawn_blocking(move || {
        let audit = path
            .as_deref()
            .ok_or_else(|| "Hubstaff audit log location is unknown.".to_owned())
            .and_then(read_audit);
        intervals
            .iter()
            .filter_map(|interval| {
                let ended_at = interval.ended_at?;
                let (mut state, mut evidence, mut excluded_ranges) = match &audit {
                    Ok(audit) => {
                        assess_audit(audit, interval.started_at, ended_at, observed_at, tracking)
                    }
                    Err(reason) => (VerificationState::Unverified, reason.clone(), Vec::new()),
                };
                let counted = interval.elapsed_end_ms? - interval.elapsed_start_ms;
                if ((ended_at - interval.started_at).num_milliseconds() - counted).abs() > 1000 {
                    state = VerificationState::Unverified;
                    evidence = "The local and Hubstaff clock boundaries differ; automatic verification will retry when sufficient evidence is available.".into();
                    excluded_ranges.clear();
                }
                Some(VerificationResult {
                    id: interval.id,
                    ended_at,
                    state,
                    evidence,
                    excluded_ranges,
                })
            })
            .collect()
    })
    .await
    .unwrap_or_else(|_| {
        fallback
            .iter()
            .filter_map(|interval| {
                Some(VerificationResult {
                    id: interval.id,
                    ended_at: interval.ended_at?,
                    state: VerificationState::Unverified,
                    evidence: "Could not read Hubstaff audit evidence; verification will retry."
                        .into(),
                    excluded_ranges: Vec::new(),
                })
            })
            .collect()
    })
}

fn read_audit(path: &Path) -> Result<String, String> {
    const LIMIT: u64 = 8 * 1024 * 1024;
    let file = std::fs::File::open(path)
        .map_err(|_| "Hubstaff audit log is unavailable; verification will retry.".to_owned())?;
    let mut text = String::new();
    file.take(LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|_| "Hubstaff audit log could not be read; verification will retry.".to_owned())?;
    if text.len() as u64 > LIMIT || !text.ends_with('\n') {
        return Err("Hubstaff audit log is incomplete or exceeds the supported size; time remains unverified.".into());
    }
    Ok(text)
}

/// Assess an interval against the observed desktop audit format. A current
/// tracking response alone is never evidence of continuity through an outage.
pub fn assess_audit(
    text: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    tracking: bool,
) -> AuditAssessment {
    let pending = |reason: &str| (VerificationState::Unverified, reason.to_owned(), Vec::new());
    if end < start || observed_at < end {
        return pending(
            "Clock boundaries could not be reconciled with Hubstaff; time remains unverified.",
        );
    }
    let mut records = Vec::new();
    for line in text.lines() {
        let state = if line.contains("(START_TRACKING)") {
            Some(true)
        } else if line.contains("(STOP_TRACKING)") {
            Some(false)
        } else if line.contains("(SHUTDOWN)") || line.contains("(STARTUP_") {
            None
        } else {
            continue;
        };
        let Some(stamp) = line.get(..31) else {
            return pending("Hubstaff audit contains an unreadable tracking timestamp.");
        };
        let Ok(at) = DateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S%.f%z") else {
            return pending("Hubstaff audit contains an unreadable tracking timestamp.");
        };
        let at =
            DateTime::from_timestamp_millis(at.timestamp_millis()).expect("parsed audit timestamp");
        if records.last().is_some_and(|(previous, _)| *previous > at) {
            return pending("Hubstaff audit timestamps moved backwards; time remains unverified.");
        }
        records.push((at, state));
    }
    let current = records
        .iter()
        .rev()
        .find(|(at, _)| *at <= observed_at)
        .and_then(|(_, state)| *state);
    if current != Some(tracking) {
        return pending("Hubstaff audit does not yet agree with the fresh status response; verification will retry.");
    }
    let mut state = records
        .iter()
        .rev()
        .find(|(at, _)| *at <= start)
        .and_then(|(_, state)| *state);
    if state.is_none() {
        return pending(
            "Hubstaff audit does not cover the start of this interval; time remains unverified.",
        );
    }
    let mut cursor = start;
    let mut unsupported_ms = 0;
    let mut excluded_ranges = Vec::new();
    for (at, next) in records.iter().filter(|(at, _)| *at > start && *at < end) {
        match state {
            Some(false) => {
                unsupported_ms += (*at - cursor).num_milliseconds();
                excluded_ranges.push((cursor, *at));
            }
            None => {
                return pending(
                    "Hubstaff restarted during the interval; continuous evidence is unavailable.",
                )
            }
            _ => {}
        }
        cursor = *at;
        state = *next;
    }
    match state {
        Some(false) => {
            unsupported_ms += (end - cursor).num_milliseconds();
            excluded_ranges.push((cursor, end));
        }
        None => {
            return pending(
                "Hubstaff restarted during the interval; continuous evidence is unavailable.",
            )
        }
        _ => {}
    }
    if unsupported_ms > 0 {
        (VerificationState::Adjusted, format!(
            "Automatically excluded {}.{:03} seconds when Hubstaff audit showed tracking stopped. The original local record was retained for reference.",
            unsupported_ms / 1000, unsupported_ms % 1000
        ), excluded_ranges)
    } else {
        (
            VerificationState::Confirmed,
            "Hubstaff local audit log supports continuous tracking throughout this interval."
                .into(),
            Vec::new(),
        )
    }
}

/// Apply evidence-backed exclusions to the original locally measured timeline.
/// Event rows stay untouched so the original accounting can be reconstructed.
pub fn reconciled_segments(
    segments: Vec<crate::breakdown::Segment>,
    intervals: &[TimeVerification],
    events: &[crate::storage::Event],
) -> Vec<crate::breakdown::Segment> {
    let mut result = Vec::new();
    for segment in segments {
        let end = segment.ended_at();
        let mut cursor = segment.started_at;
        let raw_start = events
            .iter()
            .find(|event| {
                event.task_id == segment.task_id
                    && event.occurred_at == segment.started_at
                    && event.kind.opens_interval()
            })
            .map_or(0, |event| event.elapsed_ms);
        let mut cuts: Vec<_> = intervals
            .iter()
            .filter(|interval| interval.task_id == segment.task_id)
            .flat_map(|interval| interval.excluded_elapsed_ranges())
            .map(|(from, to)| {
                (
                    segment.started_at + chrono::Duration::milliseconds(from - raw_start),
                    segment.started_at + chrono::Duration::milliseconds(to - raw_start),
                )
            })
            .filter(|(start, stop)| *stop > segment.started_at && *start < end)
            .collect();
        cuts.sort_unstable();
        for (start, stop) in cuts {
            let start = start.max(cursor).min(end);
            if start > cursor {
                let mut supported = segment.clone();
                supported.started_at = cursor;
                supported.duration_ms = (start - cursor).num_milliseconds();
                result.push(supported);
            }
            cursor = cursor.max(stop.min(end));
        }
        if cursor < end {
            let mut supported = segment;
            supported.started_at = cursor;
            supported.duration_ms = (end - cursor).num_milliseconds();
            result.push(supported);
        }
    }
    result.sort_by_key(|segment| segment.started_at);
    result
}

pub fn excluded_before(intervals: &[TimeVerification], task_id: &str, raw_elapsed_ms: i64) -> i64 {
    intervals
        .iter()
        .filter(|interval| interval.task_id == task_id)
        .flat_map(|interval| interval.excluded_elapsed_ranges())
        .map(|(start, end)| (raw_elapsed_ms.min(end) - start).max(0))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(ms: i64) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(ms).unwrap()
    }

    fn log(records: &[(i64, &str)]) -> String {
        records
            .iter()
            .map(|(ms, marker)| {
                format!(
                    "{} [AUDIT] [main] ActivityTracker.cpp:1 ({marker}) Tracking\n",
                    at(*ms).format("%Y-%m-%d %H:%M:%S%.6f%z")
                )
            })
            .collect()
    }

    #[test]
    fn continuous_tracking_is_confirmed_with_a_fresh_response_and_audit_anchor() {
        let text = log(&[(0, "START_TRACKING")]);
        assert_eq!(
            assess_audit(&text, at(1000), at(9000), at(10000), true).0,
            VerificationState::Confirmed
        );
        assert_eq!(
            assess_audit(&text, at(1000), at(9000), at(10000), false).0,
            VerificationState::Unverified
        );
        assert_eq!(
            assess_audit("", at(1000), at(9000), at(10000), true).0,
            VerificationState::Unverified
        );
    }

    #[test]
    fn stop_restart_inside_outage_is_adjusted_even_when_tracking_now() {
        let text = log(&[
            (0, "START_TRACKING"),
            (3000, "STOP_TRACKING"),
            (5500, "START_TRACKING"),
        ]);
        let (state, evidence, ranges) = assess_audit(&text, at(1000), at(9000), at(10000), true);
        assert_eq!(state, VerificationState::Adjusted);
        assert!(evidence.contains("2.500 seconds"));
        assert_eq!(ranges, vec![(at(3000), at(5500))]);
        assert_eq!(
            assess_audit(&text, at(5500), at(9000), at(10000), true).0,
            VerificationState::Confirmed
        );
    }

    #[test]
    fn stopped_on_reconnection_reports_the_unsupported_portion() {
        let text = log(&[(0, "START_TRACKING"), (3500, "STOP_TRACKING")]);
        let (state, evidence, _) = assess_audit(&text, at(1000), at(9000), at(10000), false);
        assert_eq!(state, VerificationState::Adjusted);
        assert!(evidence.contains("5.500 seconds"));
    }

    #[test]
    fn incomplete_rotated_or_inconsistent_history_is_never_confirmed() {
        for text in [
            log(&[(2000, "START_TRACKING")]),
            log(&[
                (0, "START_TRACKING"),
                (6000, "SHUTDOWN"),
                (7000, "START_TRACKING"),
            ]),
            log(&[
                (0, "START_TRACKING"),
                (7000, "STOP_TRACKING"),
                (6000, "START_TRACKING"),
            ]),
            "bad timestamp (START_TRACKING)\n".into(),
        ] {
            assert_eq!(
                assess_audit(&text, at(1000), at(9000), at(10000), true).0,
                VerificationState::Unverified
            );
        }
        assert_eq!(
            assess_audit(
                &log(&[(0, "START_TRACKING")]),
                at(9000),
                at(1000),
                at(10000),
                true
            )
            .0,
            VerificationState::Unverified
        );
    }

    #[test]
    fn real_desktop_timestamp_format_and_offsets_are_supported() {
        let text = "2026-11-15 20:33:13.063935+0800 [AUDIT] [main] ActivityTracker.cpp:2406 (START_TRACKING) Tracking Started\n";
        let start = "2026-11-15T12:36:56.387Z".parse().unwrap();
        let end = "2026-11-15T12:39:34.623Z".parse().unwrap();
        assert_eq!(
            assess_audit(text, start, end, end, true).0,
            VerificationState::Confirmed
        );
    }

    #[test]
    fn accounting_mapping_keeps_clock_rounding_and_multiple_cuts_consistent() {
        let cuts = excluded_elapsed_ranges(
            at(1000),
            at(9000),
            1000,
            8999,
            &[(at(3000), at(5500)), (at(7000), at(8000))],
        );
        assert_eq!(cuts, [(2999, 5499), (6999, 7999)]);
        let interval = TimeVerification {
            id: 1,
            task_id: "a".into(),
            started_at: at(1000),
            ended_at: Some(at(9000)),
            elapsed_start_ms: 1000,
            elapsed_end_ms: Some(8999),
            state: VerificationState::Adjusted,
            reason: "timeout".into(),
            evidence: String::new(),
            excluded_ranges: vec![(at(3000), at(5500)), (at(7000), at(8000))],
        };
        let event = crate::storage::Event {
            id: 1,
            task_id: "a".into(),
            kind: crate::storage::EventKind::Start,
            occurred_at: at(0),
            elapsed_ms: 0,
            details: None,
            cause: None,
            related_task_id: None,
        };
        let segments = reconciled_segments(
            vec![crate::breakdown::Segment {
                task_id: "a".into(),
                started_at: at(0),
                duration_ms: 8999,
                ended_by: None,
            }],
            std::slice::from_ref(&interval),
            &[event],
        );
        assert_eq!(segments.iter().map(|s| s.duration_ms).sum::<i64>(), 5499);
        assert_eq!(excluded_before(&[interval], "a", 8999), 3500);
    }

    #[test]
    fn missing_and_partial_files_leave_a_durable_pending_result() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let interval = TimeVerification {
            id: 1,
            task_id: "a".into(),
            started_at: at(1000),
            ended_at: Some(at(9000)),
            elapsed_start_ms: 1000,
            elapsed_end_ms: Some(9000),
            state: VerificationState::Unverified,
            reason: "CLI timeout".into(),
            evidence: String::new(),
            excluded_ranges: Vec::new(),
        };
        let results = runtime.block_on(check_audit(None, vec![interval], at(10000), true));
        assert_eq!(results[0].state, VerificationState::Unverified);
        let path = std::env::temp_dir().join(format!("lbx-audit-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, log(&[(0, "START_TRACKING")]).trim_end()).unwrap();
        assert!(read_audit(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
