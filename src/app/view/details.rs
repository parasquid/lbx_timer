//! Details: the selected timer's name, notes, totals, and event log.

use crate::app::format::*;
use crate::app::metrics::*;
use crate::app::{App, Message};
use crate::widgets::select_on_focus::select_on_focus;
use iced::widget::{button, column, container, row, scrollable, text, text_input, Id, Space};
use iced::{Element, Font, Length};

impl App {
    pub(crate) fn details_panel(&self) -> Element<'_, Message> {
        let selected = self.selected_id.is_some();
        let retry: Element<'_, Message> = if self.error_message.is_some() && selected {
            button("Retry saving")
                .on_press(Message::SaveDetails)
                .height(Length::Fixed(SMALL_BUTTON_HEIGHT))
                .into()
        } else {
            Space::new().into()
        };
        let save_status = container(
            row![
                text(self.selected_totals().unwrap_or_default())
                    .size(TEXT_BODY)
                    .width(Length::Fill),
                retry,
            ]
            .spacing(SPACE_M)
            .align_y(iced::Alignment::Center),
        )
        .id(Id::new("details-totals"))
        .height(Length::Fixed(SMALL_BUTTON_HEIGHT));
        let fields: Element<'_, Message> = if self.details_editable() {
            column![
                // The placeholder is the timer's default name, so an unnamed
                // timer reads the same here as in the clock card and list.
                // Typing into an unnamed timer starts from an empty field;
                // focusing a named one selects the whole name (see
                // `select_on_focus`).
                select_on_focus(
                    text_input(&self.default_name(), &self.name)
                        .id(Id::new("task-name"))
                        .on_input(Message::NameChanged)
                        .padding(PAD_INPUT),
                ),
                text_input("Notes", &self.notes)
                    .id(Id::new("task-notes"))
                    .on_input(Message::NotesChanged)
                    .padding(PAD_INPUT),
            ]
            .spacing(SPACE_L)
            .into()
        } else {
            // A past timer is part of the record: shown, not edited.
            let notes = if self.notes.trim().is_empty() {
                "No notes"
            } else {
                self.notes.as_str()
            };
            let name = if self.name.trim().is_empty() {
                self.default_name()
            } else {
                self.name.clone()
            };
            container(
                column![
                    text(name).size(TEXT_TITLE),
                    text(notes)
                        .size(TEXT_BODY)
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::Word),
                ]
                .spacing(SPACE_S),
            )
            .id(Id::new("details-record"))
            .width(Length::Fill)
            .into()
        };
        container(
            column![
                fields,
                save_status,
                text("Events").size(TEXT_HEADING),
                scrollable(self.event_list())
                    .id(Id::new("events-scroll"))
                    .spacing(SPACE_S)
                    .width(Length::Fill)
                    .height(Length::Fixed(EVENT_LOG_HEIGHT)),
            ]
            .spacing(SPACE_L),
        )
        .id(Id::new("details-panel"))
        .padding(PAD_PAGE)
        .style(container::rounded_box)
        .width(Length::Fill)
        .into()
    }

    /// The selected timer's event log, oldest first: each entry is a full
    /// sentence, its time in the session afterwards, and when it happened.
    pub(crate) fn event_list(&self) -> Element<'_, Message> {
        if self.selected_log.is_empty() {
            return column![text("No events yet")].into();
        }
        self.selected_log
            .iter()
            .fold(column![].spacing(SPACE_L), |list, entry| {
                list.push(
                    column![
                        row![
                            text(&entry.text)
                                .size(TEXT_BODY)
                                .width(Length::Fill)
                                .wrapping(iced::widget::text::Wrapping::Word),
                            text(format_duration(entry.elapsed_ms))
                                .font(Font::MONOSPACE)
                                .size(TEXT_BODY)
                                .width(Length::Fixed(TIME_COLUMN_SMALL_WIDTH))
                                .align_x(iced::widget::text::Alignment::Right),
                        ]
                        .spacing(SPACE_M),
                        text(log_time(entry)).size(TEXT_META),
                    ]
                    .spacing(SPACE_2XS),
                )
            })
            .into()
    }

    /// "This session 00:40:10 · 02:11:05 in total across 2 sessions" for the
    /// selected timer (the second part only for work that continues earlier
    /// sessions).
    pub(crate) fn selected_totals(&self) -> Option<String> {
        let id = self.selected_id.as_deref()?;
        let (elapsed, total) = match self.engine.as_ref()?.timer(id) {
            Some(timer) => (
                timer.elapsed_ms,
                timer
                    .is_continued()
                    .then(|| (timer.total_ms(), timer.total_sessions())),
            ),
            None => {
                let task = self.history_task(id)?;
                let series = self.series_totals.get(&task.series_id).copied();
                (
                    task.elapsed_ms,
                    series
                        .filter(|series| series.sessions > 1)
                        .map(|series| (series.total_ms, series.sessions)),
                )
            }
        };
        let this = format!("This session {}", format_duration(elapsed));
        Some(match total {
            Some((total_ms, sessions)) => format!(
                "{this} · {} in total across {sessions} {}",
                format_duration(total_ms),
                if sessions == 1 { "session" } else { "sessions" }
            ),
            None => this,
        })
    }
}
