//! The linked timer list: one row per timer with its play/pause button.

use super::widgets::*;
use crate::app::format::*;
use crate::app::metrics::*;
use crate::app::style::tooltip_style;
use crate::app::{App, Message};
use crate::widgets::icons::{icon, Icon};
use iced::widget::{button, column, container, row, scrollable, text, tooltip, Id, Space};
use iced::{Element, Font, Length};
use lbx_timer::{Timer, TimerState, MAX_TIMERS};

impl App {
    /// The linked timer set. Its rows behave as a radio group: starting any
    /// timer pauses whichever timer was running.
    pub(crate) fn timers_panel(&self) -> Element<'_, Message> {
        let Some(engine) = self.engine.as_ref() else {
            return Space::new().into();
        };
        let timers = engine.timers();
        let focused = self.focused_timer().map(|timer| timer.id.as_str());
        let can_remove = timers.len() > 1;

        let rows = timers
            .iter()
            .fold(column![].spacing(TIMER_ROW_SPACING), |list, timer| {
                list.push(self.timer_row(timer, focused, can_remove))
            });
        let visible = timers.len().clamp(1, VISIBLE_TIMER_ROWS) as f32;
        let list_height = visible * TIMER_ROW_HEIGHT + (visible - 1.0) * TIMER_ROW_SPACING;
        let timer_list = scrollable(rows)
            .id(Id::new("timers-scroll"))
            .height(Length::Fixed(list_height));
        // Reserve room for the scrollbar only once the list actually scrolls,
        // so it never covers the row buttons.
        let timer_list = if timers.len() > VISIBLE_TIMER_ROWS {
            timer_list.spacing(SPACE_S)
        } else {
            timer_list
        };

        let add = button("+ Add timer")
            .on_press_maybe((timers.len() < MAX_TIMERS).then_some(Message::AddTimer))
            .style(button::secondary)
            .height(Length::Fixed(SMALL_BUTTON_HEIGHT));
        // Continuations are unique by series in the current session. Summing
        // each timer's corrected total includes its history exactly once.
        let cumulative: Element<'_, Message> = if timers.iter().any(Timer::is_continued) {
            container(
                row![
                    text("Across sessions").size(TEXT_CAPTION),
                    container(
                        text(format_duration(displayed_total(
                            timers.iter().map(Timer::total_ms)
                        )))
                        .font(Font::MONOSPACE)
                        .size(TEXT_CAPTION)
                    )
                    .id(Id::new("session-total-across-sessions"))
                    .width(Length::Fixed(TIME_COLUMN_WIDTH))
                    .align_x(iced::Alignment::End),
                ]
                .spacing(SPACE_M)
                .align_y(iced::Alignment::Center),
            )
            .center_y(Length::Fixed(SESSION_TOTAL_CAPTION_HEIGHT))
            .into()
        } else {
            Space::new()
                .height(Length::Fixed(SESSION_TOTAL_CAPTION_HEIGHT))
                .into()
        };
        container(
            column![
                row![
                    text("Timers").size(TEXT_DEFAULT).width(Length::Fill),
                    column![
                        row![
                            text("This session").size(TEXT_BODY),
                            container(
                                text(format_duration(displayed_total(
                                    timers.iter().map(|timer| timer.elapsed_ms)
                                )))
                                .font(Font::MONOSPACE)
                                .size(TEXT_DEFAULT)
                            )
                            .id(Id::new("session-total"))
                            .width(Length::Fixed(TIME_COLUMN_WIDTH))
                            .align_x(iced::Alignment::End),
                        ]
                        .spacing(SPACE_M)
                        .align_y(iced::Alignment::Center)
                        .height(Length::Fixed(
                            TIMERS_HEADER_HEIGHT - SESSION_TOTAL_CAPTION_HEIGHT
                        )),
                        cumulative,
                    ]
                    .align_x(iced::Alignment::End),
                ]
                .spacing(SPACE_M)
                .align_y(iced::Alignment::Center)
                .height(Length::Fixed(TIMERS_HEADER_HEIGHT)),
                timer_list,
                container(add).id(Id::new("add-timer")),
            ]
            .spacing(SPACE_XS),
        )
        .id(Id::new("timers-panel"))
        .padding(PAD_PANEL)
        .style(container::rounded_box)
        .width(Length::Fill)
        .into()
    }

    /// One timer row: its name and time (click to focus), its play/pause
    /// button, and × for a timer that has never run.
    fn timer_row<'a>(
        &'a self,
        timer: &'a Timer,
        focused: Option<&str>,
        can_remove: bool,
    ) -> Element<'a, Message> {
        let running = timer.state == TimerState::Running;
        let can_start = self.can_start_timer();
        let label = button(
            row![
                swatch(timer.number),
                text(compact_row_title(&timer.label()))
                    .width(Length::Fill)
                    .wrapping(iced::widget::text::Wrapping::None),
                elapsed_with_total(
                    timer.elapsed_ms,
                    timer.is_continued().then(|| timer.total_ms()),
                    TEXT_DEFAULT,
                ),
            ]
            .spacing(SPACE_M)
            .height(Length::Fill)
            .align_y(iced::Alignment::Center),
        )
        .on_press(Message::FocusTimer(timer.id.clone()))
        .style(if running {
            button::success
        } else if focused == Some(timer.id.as_str()) {
            button::primary
        } else {
            button::secondary
        })
        .width(Length::Fill)
        .height(Length::Fixed(TIMER_ROW_HEIGHT));
        // One click starts any timer; the running one is paused in
        // the same step, like a radio group.
        let (glyph, hint, toggle_message) = match timer.state {
            TimerState::Running => (Icon::Pause, "Pause", Some(Message::Pause)),
            TimerState::Paused => (
                Icon::Play,
                "Resume",
                can_start.then_some(Message::Start(timer.id.clone())),
            ),
            TimerState::Unstarted => (
                Icon::Play,
                "Start",
                can_start.then_some(Message::Start(timer.id.clone())),
            ),
        };
        let toggle = tooltip(
            button(
                container(icon(glyph, TIMER_ICON_SIZE))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .on_press_maybe(toggle_message)
            .style(button::secondary)
            .padding(0)
            .width(Length::Fill)
            .height(Length::Fixed(TIMER_ROW_HEIGHT)),
            container(text(hint).size(TEXT_SMALL)).padding([SPACE_2XS, SPACE_S]),
            tooltip::Position::Top,
        )
        .style(tooltip_style)
        .gap(SPACE_XS)
        // Wait before showing, so moving across the column of buttons
        // does not flash a label over each one.
        .delay(TOOLTIP_DELAY);
        let remove: Element<'_, Message> = if timer.state == TimerState::Unstarted && can_remove {
            button(centered("×", TEXT_REMOVE_GLYPH))
                .on_press(Message::RemoveTimer(timer.id.clone()))
                .style(button::text)
                .width(Length::Fill)
                .height(Length::Fixed(TIMER_ROW_HEIGHT))
                .into()
        } else {
            Space::new().into()
        };
        container(
            row![
                label,
                container(toggle)
                    .id(format!("timer-toggle-{}", timer.id))
                    .width(Length::Fixed(TIMER_TOGGLE_WIDTH)),
                container(remove)
                    .id(format!("timer-remove-{}", timer.id))
                    .width(Length::Fixed(TIMER_REMOVE_WIDTH)),
            ]
            .spacing(SPACE_S)
            .align_y(iced::Alignment::Center),
        )
        .id(format!("timer-row-{}", timer.id))
        .width(Length::Fill)
        .height(Length::Fixed(TIMER_ROW_HEIGHT))
        .into()
    }
}
