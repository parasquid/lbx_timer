//! Colors and widget styles.

use crate::app::metrics::{BORDER_WIDTH, SWATCH_RADIUS, TOOLTIP_RADIUS};
use iced::widget::container;
use iced::{Border, Color, Theme};
use lbx_timer::Task as DbTask;

/// Colors link a timer's list row, its share of the bar, and its history row.
pub(crate) const TIMER_COLORS: [Color; 9] = [
    Color::from_rgb(0.36, 0.55, 0.94),
    Color::from_rgb(0.88, 0.64, 0.35),
    Color::from_rgb(0.37, 0.75, 0.56),
    Color::from_rgb(0.78, 0.47, 0.87),
    Color::from_rgb(0.88, 0.42, 0.46),
    Color::from_rgb(0.34, 0.71, 0.76),
    Color::from_rgb(0.82, 0.60, 0.40),
    Color::from_rgb(0.60, 0.76, 0.47),
    Color::from_rgb(0.67, 0.70, 0.75),
];

pub(crate) fn timer_color(number: i64) -> Color {
    TIMER_COLORS[((number.max(1) - 1) as usize) % TIMER_COLORS.len()]
}

pub(crate) fn task_color(task: &DbTask) -> Color {
    timer_color(task.position)
}

/// Hover labels stand out from the panel they float over.
pub(crate) fn tooltip_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.strong.color.into()),
        text_color: Some(palette.background.strong.text),
        border: Border::default()
            .rounded(TOOLTIP_RADIUS)
            .width(BORDER_WIDTH)
            .color(palette.background.weak.color),
        ..container::Style::default()
    }
}

pub(crate) fn color_block(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(color.into()),
        border: Border::default().rounded(SWATCH_RADIUS),
        ..container::Style::default()
    }
}

pub(crate) fn report_backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.65).into()),
        ..container::Style::default()
    }
}
