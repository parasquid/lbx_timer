//! History: one entry per session, with its breakdown and links.

use super::widgets::*;
use crate::app::format::*;
use crate::app::metrics::*;
use crate::app::style::*;
use crate::app::{App, Message};
use iced::widget::{button, column, container, row, scrollable, text, Id, Space};
use iced::{Color, Element, Font, Length};
use lbx_timer::breakdown::{self, TimelineEntry};
use lbx_timer::SessionSummary;
use std::collections::HashMap;

impl App {
    /// History grouped by linked session: one row per session with the total
    /// across its timers; expanding it shows each timer's share and the
    /// interleaved timeline of switches.
    /// History's panel; `list_height` is fixed in the single-column layout
    /// and fills the pane in the wide layout.
    pub(crate) fn history_panel(&self, list_height: Length) -> Element<'_, Message> {
        // A filling list needs a filling panel; a fixed one sizes to it.
        let panel_height = if list_height == Length::Fill {
            Length::Fill
        } else {
            Length::Shrink
        };
        let current_session = self.current_session_id();
        let sessions = self
            .history
            .iter()
            .fold(column![].spacing(SPACE_S), |list, summary| {
                let key = summary.id().to_owned();
                let expanded = self.expanded_sessions.contains(&key);
                let is_current = Some(summary.id()) == current_session;
                let count = summary.tasks.len();
                let header = button(
                    column![
                        text({
                            let title = session_title(summary, is_current);
                            let label = summary
                                .tasks
                                .iter()
                                .find_map(|task| self.verification_label(Some(&task.id)));
                            label.map_or(title.clone(), |label| format!("{label} · {title}"))
                        })
                        .width(Length::Fill)
                        .height(Length::Fixed(HISTORY_TITLE_HEIGHT))
                        .wrapping(iced::widget::text::Wrapping::None),
                        row![
                            text(format_duration(displayed_total(
                                summary.tasks.iter().map(|task| task.elapsed_ms)
                            )))
                            .font(Font::MONOSPACE)
                            .width(Length::Fixed(TIME_COLUMN_WIDTH))
                            .height(Length::Fixed(HISTORY_LINE_HEIGHT)),
                            Space::new().width(Length::Fill),
                            text(format!(
                                "{}{count} {} {}",
                                self.link_tag(summary)
                                    .map(|tag| format!("{tag} · "))
                                    .unwrap_or_default(),
                                if count == 1 { "timer" } else { "timers" },
                                if expanded { "▴" } else { "▾" }
                            ))
                            .height(Length::Fixed(HISTORY_LINE_HEIGHT))
                            .align_x(iced::widget::text::Alignment::Right),
                        ]
                        .height(Length::Fixed(HISTORY_LINE_HEIGHT)),
                    ]
                    .spacing(SPACE_2XS)
                    .width(Length::Fill)
                    .height(Length::Fixed(HISTORY_ROW_CONTENT_HEIGHT)),
                )
                .on_press(Message::ToggleSession(key.clone()))
                .style(if expanded {
                    button::primary
                } else {
                    button::secondary
                })
                .width(Length::Fill)
                .height(Length::Fixed(HISTORY_ROW_HEIGHT));
                let list = list.push(
                    container(header)
                        .id(format!("history-row-{key}"))
                        .width(Length::Fill)
                        .height(Length::Fixed(HISTORY_ROW_HEIGHT)),
                );
                if expanded {
                    list.push(self.session_breakdown(summary, &key, is_current))
                } else {
                    list
                }
            });
        container(
            column![
                row![
                    Space::new().width(Length::Fill),
                    text(self.history.len().to_string())
                        .font(Font::MONOSPACE)
                        .width(Length::Fixed(COMPACT_COLUMN_WIDTH))
                        .align_x(iced::widget::text::Alignment::Right),
                ],
                scrollable(sessions)
                    .id(Id::new("history-scroll"))
                    .spacing(SPACE_S)
                    .height(list_height),
            ]
            .spacing(SPACE_M)
            .height(panel_height),
        )
        .id(Id::new("history-panel"))
        .padding(PAD_PAGE)
        .style(container::rounded_box)
        .width(Length::Fill)
        .height(panel_height)
        .into()
    }

    /// "continues Oct 09" (or "continued later") for a session that is part
    /// of work spanning several sessions.
    pub(crate) fn link_tag(&self, summary: &SessionSummary) -> Option<String> {
        if let Some(source) = summary.continues_sessions.first() {
            return Some(match self.history_session(source) {
                Some(source) => format!("continues {}", short_date(source.session.started_at)),
                None => "continues earlier work".to_owned(),
            });
        }
        (!summary.continued_by_sessions.is_empty()).then(|| "continued later".to_owned())
    }

    /// Links to the sessions this one continues or is continued by, and its
    /// Resume session button.
    pub(crate) fn session_actions(
        &self,
        summary: &SessionSummary,
        key: &str,
        is_current: bool,
    ) -> Element<'_, Message> {
        let mut links = column![].spacing(SPACE_XS).width(Length::Fill);
        let related = [
            ("Continues", &summary.continues_sessions),
            ("Continued", &summary.continued_by_sessions),
        ];
        for (verb, ids) in related {
            for id in ids {
                if let Some(other) = self.history_session(id) {
                    links = links.push(
                        button(
                            text(format!("{verb} {}", date_time(other.session.started_at)))
                                .size(TEXT_SMALL),
                        )
                        .on_press(Message::RevealSession(id.clone()))
                        .style(button::text)
                        .padding([SPACE_2XS, 0.0]),
                    );
                }
            }
        }
        let resume: Element<'_, Message> = if is_current {
            Space::new().into()
        } else {
            container(
                button(text("Resume session").size(TEXT_BODY))
                    .on_press(Message::ResumeSession(key.to_owned()))
                    .style(button::secondary),
            )
            .id(format!("resume-session-{key}"))
            .into()
        };
        let report = container(
            button(text("Report").size(TEXT_BODY))
                .on_press(Message::OpenReport(key.to_owned()))
                .style(button::secondary),
        )
        .id(format!("report-session-{key}"));
        row![links, report, resume]
            .spacing(SPACE_M)
            .align_y(iced::Alignment::Center)
            .into()
    }

    pub(crate) fn session_breakdown(
        &self,
        summary: &SessionSummary,
        key: &str,
        is_current: bool,
    ) -> Element<'_, Message> {
        let total = summary.total_ms();
        let shares = percent_shares(
            &summary
                .tasks
                .iter()
                .map(|task| task.elapsed_ms)
                .collect::<Vec<_>>(),
        );
        let mut content = column![].spacing(SPACE_S);

        if total > 0 {
            let bar = summary
                .tasks
                .iter()
                .filter(|task| task.elapsed_ms > 0)
                .fold(row![].spacing(SPACE_2XS), |bar, task| {
                    bar.push(
                        container(Space::new())
                            .width(Length::FillPortion(share_portion(task.elapsed_ms, total)))
                            .height(Length::Fixed(SHARE_BAR_HEIGHT))
                            .style(color_block(task_color(task))),
                    )
                });
            content = content.push(container(bar).id(format!("share-bar-{key}")));
        }
        content = content.push(self.session_actions(summary, key, is_current));

        for (task, share) in summary.tasks.iter().zip(&shares) {
            let selected = self.selected_id.as_deref() == Some(task.id.as_str());
            let select = button(
                row![
                    swatch_color(task_color(task)),
                    text(compact_history_title(&task.label()))
                        .size(TEXT_BODY)
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::None),
                    elapsed_with_total(
                        task.elapsed_ms,
                        self.series_totals
                            .get(&task.series_id)
                            .filter(|series| series.sessions > 1)
                            .map(|series| series.total_ms),
                        TEXT_BODY,
                    ),
                    text(share.map_or_else(|| "—".to_owned(), |share| format!("{share}%")))
                        .size(TEXT_BODY)
                        .width(Length::Fixed(SHARE_COLUMN_WIDTH))
                        .align_x(iced::widget::text::Alignment::Right),
                ]
                .spacing(SPACE_S)
                .height(Length::Fill)
                .align_y(iced::Alignment::Center),
            )
            .on_press(Message::SelectTask(task.id.clone()))
            .style(if selected {
                button::primary
            } else {
                button::secondary
            })
            .width(Length::Fill)
            .height(Length::Fixed(HISTORY_TASK_ROW_HEIGHT));
            content = content.push(
                container(select)
                    .id(format!("history-task-{}", task.id))
                    .width(Length::Fill)
                    .height(Length::Fixed(HISTORY_TASK_ROW_HEIGHT)),
            );
        }

        let segments = self.breakdowns.get(key).map(Vec::as_slice).unwrap_or(&[]);
        if !segments.is_empty() {
            let labels: HashMap<&str, (String, Color)> = summary
                .tasks
                .iter()
                .map(|task| (task.id.as_str(), (task.label(), task_color(task))))
                .collect();
            let timeline = breakdown::timeline(segments, TIMELINE_MIN_GAP_MS)
                .into_iter()
                .fold(
                    column![text("Timeline").size(TEXT_BODY)].spacing(SPACE_2XS),
                    |list, entry| {
                        let (start, label, color, duration) = match entry {
                            TimelineEntry::Running(segment) => {
                                let (label, color) = labels
                                    .get(segment.task_id.as_str())
                                    .cloned()
                                    .unwrap_or_else(|| ("Deleted timer".into(), TIMER_COLORS[8]));
                                (segment.started_at, label, Some(color), segment.duration_ms)
                            }
                            TimelineEntry::Idle {
                                started_at,
                                duration_ms,
                            } => (started_at, "Paused".to_owned(), None, duration_ms),
                        };
                        let marker: Element<'_, Message> = match color {
                            Some(color) => swatch_color(color),
                            None => Space::new().width(Length::Fixed(SWATCH_SIZE)).into(),
                        };
                        list.push(
                            row![
                                text(clock_time(start))
                                    .font(Font::MONOSPACE)
                                    .size(TEXT_SMALL)
                                    .width(Length::Fixed(COMPACT_COLUMN_WIDTH)),
                                marker,
                                text(compact_history_title(&label))
                                    .size(TEXT_SMALL)
                                    .width(Length::Fill)
                                    .wrapping(iced::widget::text::Wrapping::None),
                                text(format_duration(duration))
                                    .font(Font::MONOSPACE)
                                    .size(TEXT_SMALL)
                                    .width(Length::Fixed(TIME_COLUMN_SMALL_WIDTH))
                                    .align_x(iced::widget::text::Alignment::Right),
                            ]
                            .spacing(SPACE_S)
                            .align_y(iced::Alignment::Center)
                            .height(Length::Fixed(HISTORY_LINE_HEIGHT)),
                        )
                    },
                );
            content = content.push(container(timeline).id(format!("timeline-{key}")));
        }

        container(content)
            .id(format!("session-detail-{key}"))
            .padding([SPACE_S, 0.0])
            .width(Length::Fill)
            .into()
    }
}
