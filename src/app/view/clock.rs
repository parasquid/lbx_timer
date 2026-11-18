//! The clock card: the focused timer's time, state, and main actions.

use crate::app::format::*;
use crate::app::metrics::*;
use crate::app::{App, Message};
use iced::widget::{button, column, container, row, text, Id};
use iced::{Element, Font, Length};
use lbx_timer::TimerState;

impl App {
    /// The large clock for the focused timer, with its primary action and
    /// End session. Every state uses the same slots so nothing moves.
    pub(crate) fn active_panel(&self) -> Element<'_, Message> {
        let can_start = self.can_start_timer();
        let focused = self.focused_timer();
        let (state, elapsed_ms, title, action) = match focused {
            Some(timer) => {
                let action = match timer.state {
                    TimerState::Running => button("Pause").on_press(Message::Pause),
                    TimerState::Paused => button("Resume")
                        .on_press_maybe(can_start.then_some(Message::Start(timer.id.clone()))),
                    TimerState::Unstarted => button("Start")
                        .on_press_maybe(can_start.then_some(Message::Start(timer.id.clone()))),
                };
                let state = match timer.state {
                    TimerState::Running => "RUNNING",
                    TimerState::Paused => "PAUSED",
                    TimerState::Unstarted => "IDLE",
                };
                // A continued timer's clock shows this session; the state line
                // carries the running total of the whole piece of work.
                let mut state = if timer.is_continued() {
                    format!("{state} · {} in total", format_duration(timer.total_ms()))
                } else {
                    state.to_owned()
                };
                if let Some(label) = self.verification_label(Some(&timer.id)) {
                    state = format!("{state} · {}", label.to_uppercase());
                }
                (
                    state,
                    timer.elapsed_ms,
                    compact_title(&timer.label()),
                    action,
                )
            }
            None => (
                "IDLE".to_owned(),
                0,
                String::new(),
                button("Start").on_press_maybe(can_start.then_some(Message::Start(String::new()))),
            ),
        };

        let has_time = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.session().has_recorded_time());
        let end = button("End session")
            .style(button::secondary)
            .on_press_maybe(has_time.then_some(Message::EndSession));
        let action = container(
            action
                .width(Length::Fill)
                .height(Length::Fixed(BUTTON_HEIGHT)),
        )
        .id(Id::new("primary-action"))
        .width(Length::Fill)
        .height(Length::Fixed(BUTTON_HEIGHT));
        let end = container(end.width(Length::Fill).height(Length::Fixed(BUTTON_HEIGHT)))
            .id(Id::new("end-session-action"))
            .width(Length::Fill)
            .height(Length::Fixed(BUTTON_HEIGHT));

        container(
            column![
                text(state)
                    .size(TEXT_BODY)
                    .width(Length::Fill)
                    .height(Length::Fixed(CLOCK_STATE_HEIGHT))
                    .center(),
                text(format_duration(elapsed_ms))
                    .font(Font::MONOSPACE)
                    .size(TEXT_CLOCK)
                    .width(Length::Fill)
                    .height(Length::Fixed(CLOCK_HEIGHT))
                    .center(),
                text(title)
                    .size(TEXT_TITLE)
                    .width(Length::Fill)
                    .height(Length::Fixed(CLOCK_TITLE_HEIGHT))
                    .center(),
                row![action, end]
                    .spacing(SPACE_L)
                    .width(Length::Fill)
                    .height(Length::Fixed(BUTTON_HEIGHT)),
            ]
            .spacing(SPACE_M)
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .id(Id::new("active-panel"))
        .padding(PAD_PAGE)
        .style(container::rounded_box)
        .width(Length::Fill)
        .height(Length::Fixed(ACTIVE_PANEL_HEIGHT))
        .into()
    }
}
