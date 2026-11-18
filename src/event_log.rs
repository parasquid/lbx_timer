//! Human-readable event log.
//!
//! Each line is self-contained: it names the timer, what happened to it, the
//! other timer involved in a switch, and why an automatic pause happened. Read
//! in order, the lines are enough to reconstruct the session.

use crate::storage::{Event, EventCause, EventKind};
use chrono::{DateTime, Local, Utc};

/// The earlier timer that a timer continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continuation {
    /// Its current display name.
    pub name: String,
    /// When its session started; `None` if that session is gone.
    pub session_started_at: Option<DateTime<Utc>>,
}

impl Continuation {
    /// `“Review” from the Oct 09 session`, in local time.
    fn describe(&self) -> String {
        match self.session_started_at {
            Some(started) => format!(
                "{} from the {} session",
                quoted(&self.name),
                started.with_timezone(&Local).format("%b %d")
            ),
            None => quoted(&self.name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub occurred_at: DateTime<Utc>,
    /// What happened, e.g. `Paused “Review” — switched to “Email”`.
    pub text: String,
    /// The timer's time in its session right after the event.
    pub elapsed_ms: i64,
}

/// Describe one event of the timer called `timer`.
///
/// `related` is the current name of the other timer in a switch (`None` if
/// there was none, or if it has been deleted). `continues` is the earlier
/// timer this one continues; it is mentioned on the start event.
pub fn entry(
    event: &Event,
    timer: &str,
    related: Option<&str>,
    continues: Option<&Continuation>,
) -> LogEntry {
    let name = quoted(timer);
    let other = || related.map_or_else(|| "a timer that was since deleted".to_owned(), quoted);
    let switch = event.cause == Some(EventCause::Switch);
    let mut reasons: Vec<String> = Vec::new();
    let action = match event.kind {
        EventKind::Start => {
            if switch {
                reasons.push(format!("switched from {}", other()));
            }
            if let Some(source) = continues {
                reasons.push(format!("continues {}", source.describe()));
            }
            "Started"
        }
        EventKind::Resume => {
            if switch {
                reasons.push(format!("switched from {}", other()));
            }
            "Resumed"
        }
        EventKind::Pause => {
            if switch {
                reasons.push(format!("switched to {}", other()));
            }
            "Paused"
        }
        EventKind::SleepPause => {
            reasons.push(
                "the computer slept or its clock jumped; that time was not counted".to_owned(),
            );
            "Paused"
        }
        EventKind::RecoveryPause => {
            reasons.push(
                "the app had stopped unexpectedly; time after its last save was not counted"
                    .to_owned(),
            );
            "Paused"
        }
        EventKind::ClosePause => {
            reasons.push("the window was closed".to_owned());
            "Paused"
        }
        EventKind::Stop => {
            if event.cause == Some(EventCause::EndSession) {
                reasons.push("the session was ended manually in LBX".to_owned());
            }
            "Stopped"
        }
    };
    if matches!(
        event.kind,
        EventKind::Start | EventKind::Resume | EventKind::Pause
    ) {
        reasons.push(
            event
                .details
                .as_deref()
                .filter(|detail| !detail.is_empty())
                .unwrap_or("trigger not recorded (older event)")
                .to_owned(),
        );
    }
    let text = if reasons.is_empty() {
        format!("{action} {name}")
    } else {
        format!("{action} {name} — {}", reasons.join("; "))
    };
    LogEntry {
        occurred_at: event.occurred_at,
        text,
        elapsed_ms: event.elapsed_ms,
    }
}

fn quoted(name: &str) -> String {
    format!("“{name}”")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn event(kind: EventKind, cause: Option<EventCause>, related: Option<&str>) -> Event {
        Event {
            id: 1,
            task_id: "a".into(),
            kind,
            occurred_at: Utc.timestamp_millis_opt(0).unwrap(),
            elapsed_ms: 1_000,
            details: None,
            cause,
            related_task_id: related.map(str::to_owned),
        }
    }

    #[test]
    fn every_line_names_the_timer_the_other_timer_and_the_reason() {
        let text = |kind, cause, related: Option<&str>, continues: Option<&Continuation>| {
            entry(&event(kind, cause, related), "Review", related, continues).text
        };
        let source = Continuation {
            name: "Review".into(),
            session_started_at: None,
        };
        let switch = Some(EventCause::Switch);
        assert_eq!(
            text(EventKind::Start, None, None, None),
            "Started “Review” — trigger not recorded (older event)"
        );
        assert_eq!(
            text(EventKind::Start, switch, Some("Email"), None),
            "Started “Review” — switched from “Email”; trigger not recorded (older event)"
        );
        assert_eq!(
            text(EventKind::Start, None, None, Some(&source)),
            "Started “Review” — continues “Review”; trigger not recorded (older event)"
        );
        let dated = Continuation {
            session_started_at: Some(Utc.timestamp_millis_opt(0).unwrap()),
            ..source.clone()
        };
        let line = text(EventKind::Start, None, None, Some(&dated));
        assert!(
            line.starts_with("Started “Review” — continues “Review” from the ")
                && line.ends_with(" session; trigger not recorded (older event)"),
            "{line}"
        );
        assert_eq!(
            text(EventKind::Pause, switch, Some("Email"), None),
            "Paused “Review” — switched to “Email”; trigger not recorded (older event)"
        );
        assert_eq!(
            text(EventKind::Resume, switch, Some("QA"), None),
            "Resumed “Review” — switched from “QA”; trigger not recorded (older event)"
        );
        assert_eq!(
            text(EventKind::Pause, None, None, None),
            "Paused “Review” — trigger not recorded (older event)"
        );
        assert_eq!(
            text(EventKind::Stop, Some(EventCause::EndSession), None, None),
            "Stopped “Review” — the session was ended manually in LBX"
        );
        assert!(text(EventKind::SleepPause, None, None, None).contains("slept"));
        assert!(text(EventKind::RecoveryPause, None, None, None).contains("unexpectedly"));
        assert!(text(EventKind::ClosePause, None, None, None).contains("window was closed"));
    }

    #[test]
    fn a_deleted_switch_partner_is_still_reported() {
        let line = entry(
            &event(EventKind::Pause, Some(EventCause::Switch), None),
            "Review",
            None,
            None,
        );
        assert_eq!(
            line.text,
            "Paused “Review” — switched to a timer that was since deleted; trigger not recorded (older event)"
        );
        assert_eq!(line.elapsed_ms, 1_000);
    }
}
