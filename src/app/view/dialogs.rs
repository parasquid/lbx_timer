//! Modal dialogs.

use super::widgets::*;
use crate::app::format::*;
use crate::app::metrics::*;
use crate::app::{App, Message};
use iced::widget::{button, column, container, opaque, text, Id};
use iced::{Element, Length};
use lbx_timer::LoadMode;

impl App {
    /// Shown when resuming a session while the current one has recorded time.
    pub(crate) fn resume_confirmation(&self) -> Element<'_, Message> {
        let Some(summary) = self
            .pending_resume
            .as_deref()
            .and_then(|id| self.history_session(id))
        else {
            return nothing();
        };
        let action = |label: &'static str, message: Message, id: &'static str, primary: bool| {
            container(
                button(centered(label, TEXT_BUTTON))
                    .on_press(message)
                    .style(if primary {
                        button::primary
                    } else {
                        button::secondary
                    })
                    .width(Length::Fill)
                    .height(Length::Fixed(DIALOG_BUTTON_HEIGHT)),
            )
            .id(Id::new(id))
            .width(Length::Fill)
        };
        let modal = container(
            column![
                text("Resume this session?").size(TEXT_DIALOG_TITLE),
                text(session_title(summary, false)).width(Length::Fill),
                text(
                    "The current session already has recorded time. Continue these \
                     timers alongside it, or end it first."
                )
                .size(TEXT_BODY)
                .width(Length::Fill),
                action(
                    "Add to current session",
                    Message::ConfirmResume(LoadMode::AddToCurrent),
                    "resume-add",
                    true,
                ),
                action(
                    "End current session first",
                    Message::ConfirmResume(LoadMode::EndCurrentFirst),
                    "resume-end-first",
                    false,
                ),
                action("Cancel", Message::CancelResume, "resume-cancel", false),
            ]
            .spacing(SPACE_L)
            .width(Length::Fill),
        )
        .padding(PAD_PAGE)
        .width(Length::Fixed(DIALOG_WIDTH))
        .style(container::rounded_box);

        opaque(
            container(modal)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
    }
}
