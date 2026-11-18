//! Read-only aggregate report for one selected session and its continuations.

use super::widgets::nothing;
use crate::app::format::*;
use crate::app::metrics::*;
use crate::app::{App, Message};
use iced::widget::{button, column, container, opaque, row, scrollable, text, Id, Space};
use iced::{Element, Font, Length};

impl App {
    pub(crate) fn session_report(&self) -> Element<'_, Message> {
        let Some(snapshot) = self.report.as_ref() else {
            return nothing();
        };
        let report = &snapshot.data;
        let shares = percent_shares(
            &report
                .timers
                .iter()
                .map(|timer| timer.elapsed_ms)
                .collect::<Vec<_>>(),
        );
        let mut body = column![].spacing(SPACE_2XL);
        for (timer, share) in report.timers.iter().zip(shares) {
            let mut entry = column![row![
                text(&timer.name)
                    .size(TEXT_DEFAULT)
                    .width(Length::Fill)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                text(format_duration(timer.elapsed_ms))
                    .font(Font::MONOSPACE)
                    .size(TEXT_BODY),
                text(share.map_or_else(|| "—".into(), |value| format!("{value}%")))
                    .size(TEXT_BODY)
                    .width(Length::Fixed(SHARE_COLUMN_WIDTH))
                    .align_x(iced::widget::text::Alignment::Right),
            ]
            .spacing(SPACE_M)
            .align_y(iced::Alignment::Start),]
            .spacing(SPACE_S);
            for note in &timer.notes {
                entry = entry.push(
                    text(note)
                        .size(TEXT_BODY)
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                );
            }
            body = body.push(
                container(entry)
                    .id(format!("report-timer-{}", timer.series_id))
                    .width(Length::Fill),
            );
        }
        body = body.push(
            container(
                button(
                    text(if snapshot.show_recordings {
                        "Recording details ▾"
                    } else {
                        "Recording details ▸"
                    })
                    .size(TEXT_BODY),
                )
                .on_press(Message::ToggleReportRecordings)
                .style(button::text),
            )
            .id(Id::new("report-recordings-toggle")),
        );
        if snapshot.show_recordings {
            for recording in &report.recordings {
                body = body.push(
                    container(
                        row![
                            text(if recording.session.ended_at.is_none() {
                                format!("Current · {}", date_time(recording.session.started_at))
                            } else {
                                date_time(recording.session.started_at)
                            })
                            .size(TEXT_SMALL)
                            .width(Length::Fill),
                            text(format_duration(displayed_total(
                                recording.tasks.iter().map(|task| task.elapsed_ms)
                            )))
                            .font(Font::MONOSPACE)
                            .size(TEXT_SMALL),
                        ]
                        .spacing(SPACE_M),
                    )
                    .id(format!("report-recording-{}", recording.id()))
                    .width(Length::Fill),
                );
            }
        }
        let warning: Element<'_, Message> = if snapshot.unverified {
            text("Includes Unverified time awaiting Hubstaff evidence.")
                .size(TEXT_BODY)
                .width(Length::Fill)
                .into()
        } else {
            Space::new().into()
        };
        let action = |label, message, id, primary| {
            container(
                button(text(label).size(TEXT_BODY))
                    .on_press(message)
                    .style(if primary {
                        button::primary
                    } else {
                        button::secondary
                    })
                    .height(Length::Fixed(DIALOG_BUTTON_HEIGHT))
                    .width(Length::Fill),
            )
            .id(Id::new(id))
            .width(Length::Fill)
        };
        let modal = container(
            column![
                text("Session report").size(TEXT_DIALOG_TITLE),
                text(format!(
                    "{} to {}",
                    date_time(report.started_at()),
                    date_time(report.last_activity())
                ))
                .size(TEXT_BODY)
                .width(Length::Fill),
                text(format!(
                    "As of {}",
                    snapshot
                        .generated_at
                        .with_timezone(&chrono::Local)
                        .format("%b %d, %H:%M:%S")
                ))
                .size(TEXT_META),
                row![
                    text("Total tracked time")
                        .size(TEXT_BODY)
                        .width(Length::Fill),
                    container(
                        text(format_duration(displayed_total(
                            report.timers.iter().map(|timer| timer.elapsed_ms)
                        )))
                        .font(Font::MONOSPACE)
                        .size(TEXT_TITLE)
                    )
                    .id(Id::new("report-total")),
                ]
                .spacing(SPACE_M)
                .align_y(iced::Alignment::Center),
                warning,
                scrollable(body)
                    .id(Id::new("report-scroll"))
                    .height(Length::Fill)
                    .spacing(SPACE_S),
                row![
                    action("Copy report", Message::CopyReport, "report-copy", true),
                    action("Refresh", Message::RefreshReport, "report-refresh", false),
                    action("Close", Message::CloseReport, "report-close", false),
                ]
                .spacing(SPACE_M),
            ]
            .spacing(SPACE_L),
        )
        .id(Id::new("session-report"))
        .padding(PAD_PAGE)
        .width(Length::Fill)
        .max_width(REPORT_WIDTH)
        .height(Length::Fill)
        .max_height(REPORT_MAX_HEIGHT)
        .style(container::rounded_box);
        opaque(
            container(modal)
                .padding(PAD_PAGE)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(crate::app::style::report_backdrop),
        )
    }
}
