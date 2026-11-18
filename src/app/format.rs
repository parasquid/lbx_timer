//! Pure text formatting for the views.

use chrono::{DateTime, Local, Utc};
use lbx_timer::{LogEntry, SessionEnd, SessionSummary};

/// Longest title, in characters, in each place a timer or session name is
/// shown; longer ones end in "…".
const CLOCK_TITLE_CHARS: usize = 36;
const ROW_TITLE_CHARS: usize = 22;
const HISTORY_TITLE_CHARS: usize = 28;
/// Bar segments are weighted in thousandths of the session.
const SHARE_PORTION_SCALE: i64 = 1_000;

const DATE: &str = "%b %d";
const TIME: &str = "%H:%M";
const DATE_TIME: &str = "%b %d, %H:%M";
const DATE_TIME_SECONDS: &str = "%b %d, %H:%M:%S";

/// "Oct 09", in local time.
pub(crate) fn short_date(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format(DATE).to_string()
}

/// "Oct 09, 14:05", in local time.
pub(crate) fn date_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format(DATE_TIME).to_string()
}

/// "14:05", in local time.
pub(crate) fn clock_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format(TIME).to_string()
}

pub(crate) fn format_duration(milliseconds: i64) -> String {
    let total_seconds = milliseconds.max(0) / 1_000;
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// A total that matches the rows shown above it: the sum of each part's
/// whole seconds, as displayed by [`format_duration`]. Summing raw
/// milliseconds could show, say, 3 s + 3 s = 7 s.
pub(crate) fn displayed_total(parts_ms: impl IntoIterator<Item = i64>) -> i64 {
    parts_ms
        .into_iter()
        .map(|ms| ms.max(0) / 1_000 * 1_000)
        .sum()
}

/// The same snapshot, totals, shares, and full notes shown in the report dialog.
pub(crate) fn session_report_text(
    report: &lbx_timer::report::SessionReport,
    generated_at: DateTime<Utc>,
    unverified: bool,
) -> String {
    let stamp = |at: DateTime<Utc>| {
        at.with_timezone(&Local)
            .format("%Y-%m-%d %H:%M:%S %:z")
            .to_string()
    };
    let mut output = format!(
        "Session report\nPeriod: {} to {}\nAs of: {}\nTotal tracked time: {}\n",
        stamp(report.started_at()),
        stamp(report.last_activity()),
        stamp(generated_at),
        format_duration(displayed_total(
            report.timers.iter().map(|timer| timer.elapsed_ms)
        ))
    );
    if unverified {
        output.push_str("Includes Unverified time awaiting Hubstaff evidence.\n");
    }
    let shares = percent_shares(
        &report
            .timers
            .iter()
            .map(|timer| timer.elapsed_ms)
            .collect::<Vec<_>>(),
    );
    for (timer, share) in report.timers.iter().zip(shares) {
        output.push_str(&format!(
            "\n{} — {} ({})\n",
            timer.name,
            format_duration(timer.elapsed_ms),
            share.map_or_else(|| "—".into(), |value| format!("{value}%"))
        ));
        for note in &timer.notes {
            output.push_str("  Notes:\n");
            for line in note.lines() {
                output.push_str(&format!("    {line}\n"));
            }
        }
    }
    output
}

/// Whole-percent shares that always add up to exactly 100 (largest-remainder
/// rounding). `None` for every part when there is no time at all.
pub(crate) fn percent_shares(parts_ms: &[i64]) -> Vec<Option<i64>> {
    let total: i64 = parts_ms.iter().map(|ms| ms.max(&0)).sum();
    if total <= 0 {
        return vec![None; parts_ms.len()];
    }
    let exact: Vec<(i64, i64)> = parts_ms
        .iter()
        .map(|ms| {
            let scaled = ms.max(&0) * 100;
            (scaled / total, scaled % total)
        })
        .collect();
    let mut shares: Vec<i64> = exact.iter().map(|(floor, _)| *floor).collect();
    let mut order: Vec<usize> = (0..parts_ms.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(exact[i].1));
    let missing = 100 - shares.iter().sum::<i64>();
    for &i in order.iter().take(missing.max(0) as usize) {
        shares[i] += 1;
    }
    shares.into_iter().map(Some).collect()
}

/// Bar width weight in thousandths; never zero for a timer with time.
pub(crate) fn share_portion(part_ms: i64, total_ms: i64) -> u16 {
    if total_ms <= 0 {
        return 1;
    }
    ((part_ms.max(0) * SHARE_PORTION_SCALE / total_ms).clamp(1, SHARE_PORTION_SCALE)) as u16
}

pub(crate) fn compact_title(value: &str) -> String {
    truncate_title(value, CLOCK_TITLE_CHARS)
}

pub(crate) fn compact_row_title(value: &str) -> String {
    truncate_title(value, ROW_TITLE_CHARS)
}

pub(crate) fn compact_history_title(value: &str) -> String {
    truncate_title(value, HISTORY_TITLE_CHARS)
}

pub(crate) fn session_title(summary: &SessionSummary, is_current: bool) -> String {
    let session = &summary.session;
    let start = session.started_at;
    if is_current {
        return format!("Current session · since {}", clock_time(start));
    }
    let same_day = |end: DateTime<Utc>| {
        end.with_timezone(&Local).date_naive() == start.with_timezone(&Local).date_naive()
    };
    let title = match session.ended_at {
        Some(end) if same_day(end) => format!(
            "{} · {}–{}",
            short_date(start),
            clock_time(start),
            clock_time(end)
        ),
        Some(end) => format!("{} – {}", date_time(start), date_time(end)),
        None => format!("{} · {}", short_date(start), clock_time(start)),
    };
    match session.end_reason {
        Some(SessionEnd::Recovered) => format!("{title} · recovered"),
        _ => title,
    }
}

pub(crate) fn truncate_title(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut title = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        title.pop();
        title.push('…');
    }
    title
}

pub(crate) fn log_time(entry: &LogEntry) -> String {
    entry
        .occurred_at
        .with_timezone(&Local)
        .format(DATE_TIME_SECONDS)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_totals_and_shares_add_up() {
        // Rows show 3 s and 3 s, so the total must show 6 s, not 7 s.
        assert_eq!(displayed_total([3_900, 3_200]), 6_000);
        let shares = percent_shares(&[1, 1, 1]);
        assert_eq!(shares.iter().flatten().sum::<i64>(), 100);
        assert_eq!(percent_shares(&[7, 3, 1]), [Some(64), Some(27), Some(9)]);
        assert_eq!(percent_shares(&[0, 0]), [None, None]);
        assert_eq!(percent_shares(&[5, 0]), [Some(100), Some(0)]);
    }

    #[test]
    fn titles_are_cut_with_an_ellipsis_at_the_limit() {
        assert_eq!(truncate_title("Review", 6), "Review");
        assert_eq!(truncate_title("Reviewing", 6), "Revie…");
        assert_eq!(truncate_title("Überprüfung", 4).chars().count(), 4);
    }

    #[test]
    fn bar_portions_are_never_zero_for_time() {
        assert_eq!(share_portion(1, 1_000_000), 1);
        assert_eq!(share_portion(500, 1_000), 500);
        assert_eq!(share_portion(0, 0), 1);
    }
}
