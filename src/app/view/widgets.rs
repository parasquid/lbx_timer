//! Small view pieces shared by several panels.

use crate::app::format::format_duration;
use crate::app::metrics::*;
use crate::app::style::*;
use crate::app::Message;
use iced::widget::{column, container, text, Space};
use iced::{Color, Element, Font, Length};

pub(crate) fn swatch_color<'a>(color: Color) -> Element<'a, Message> {
    container(Space::new())
        .width(Length::Fixed(SWATCH_SIZE))
        .height(Length::Fixed(SWATCH_SIZE))
        .style(color_block(color))
        .into()
}

pub(crate) fn swatch<'a>(number: i64) -> Element<'a, Message> {
    swatch_color(timer_color(number))
}

/// This session's time, with the running total of the whole piece of work in
/// a small second line when it spans several sessions.
pub(crate) fn elapsed_with_total<'a>(
    elapsed_ms: i64,
    total_ms: Option<i64>,
    size: f32,
) -> Element<'a, Message> {
    let elapsed = text(format_duration(elapsed_ms))
        .font(Font::MONOSPACE)
        .size(size)
        .width(Length::Fill)
        .align_x(iced::widget::text::Alignment::Right);
    let content: Element<'a, Message> = match total_ms {
        Some(total) => column![
            elapsed,
            text(format!("{} total", format_duration(total)))
                .size(TEXT_CAPTION)
                .width(Length::Fill)
                .align_x(iced::widget::text::Alignment::Right),
        ]
        .into(),
        None => elapsed.into(),
    };
    container(content)
        .width(Length::Fixed(TIME_WITH_TOTAL_WIDTH))
        .center_y(Length::Fill)
        .into()
}

/// A button label centered vertically in a fixed-height row button.
pub(crate) fn centered<'a>(label: &'a str, size: f32) -> Element<'a, Message> {
    text(label)
        .size(size)
        .height(Length::Fill)
        .align_y(iced::alignment::Vertical::Center)
        .into()
}

/// An empty, zero-size element: what an overlay renders while hidden.
pub(crate) fn nothing<'a>() -> Element<'a, Message> {
    Space::new()
        .width(Length::Shrink)
        .height(Length::Shrink)
        .into()
}
