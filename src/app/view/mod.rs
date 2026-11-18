//! The window's layouts and the widgets they share.

mod clock;
mod details;
mod dialogs;
mod history;
mod report;
mod timers;
mod widgets;

use super::metrics::*;
use super::{App, Message};
use iced::widget::{
    button, column, container, opaque, responsive, row, scrollable, stack, text, tooltip, Id, Space,
};
use iced::{Element, Length};
use lbx_timer::provider::HubstaffStatus;

impl App {
    pub(crate) fn view(&self) -> Element<'_, Message> {
        if let Some(error) = &self.startup_error {
            return container(
                column![
                    text("LBX Timer could not start").size(TEXT_ERROR_TITLE),
                    text(error).width(Length::Fill),
                    text("Choose a different path with: lbx-timer --db /path/to/timer.sqlite3"),
                ]
                .spacing(SPACE_2XL)
                .width(Length::Fill),
            )
            .padding(PAD_ERROR_PAGE)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        }

        let layout = responsive(move |size| {
            if size.width >= WIDE_BREAKPOINT {
                self.wide_layout()
            } else {
                self.narrow_layout()
            }
        });

        container(stack([
            layout.into(),
            self.status_banner(),
            self.resume_confirmation(),
            self.session_report(),
        ]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    pub(crate) fn title_bar(&self) -> Element<'_, Message> {
        row![
            text("LBX Timer").size(TEXT_APP_TITLE),
            Space::new().width(Length::Fill),
            self.hubstaff_badge(),
        ]
        .height(Length::Fixed(TITLE_BAR_HEIGHT))
        .align_y(iced::Alignment::Center)
        .into()
    }

    fn hubstaff_badge(&self) -> Element<'_, Message> {
        let Some(status) = self.hubstaff_status() else {
            return Space::new().into();
        };
        let (mut label, mut detail) = match status {
            HubstaffStatus::Checking => (
                "Hubstaff · Checking".to_owned(),
                "Waiting for Hubstaff's local readiness and tracking status.".to_owned(),
            ),
            HubstaffStatus::Tracking { project } => (
                format!(
                    "Hubstaff · Tracking · {}",
                    compact_project(project.as_deref())
                ),
                format!(
                    "Hubstaff reports tracking is active for {}.",
                    project.as_deref().unwrap_or("an unnamed project")
                ),
            ),
            HubstaffStatus::Stopped { project } => (
                format!(
                    "Hubstaff · Stopped · {}",
                    compact_project(project.as_deref())
                ),
                format!(
                    "Hubstaff reports tracking stopped{}.",
                    project
                        .as_deref()
                        .map(|project| format!(" for {project}"))
                        .unwrap_or_default()
                ),
            ),
            HubstaffStatus::Unavailable { reason } => (
                "Hubstaff · Unavailable".to_owned(),
                format!("Hubstaff status is unavailable: {reason}"),
            ),
        };
        if let Some(verification) = self.verification_label(None) {
            let connection = match status {
                HubstaffStatus::Tracking { .. } => "Tracking",
                HubstaffStatus::Stopped { .. } => "Stopped",
                HubstaffStatus::Checking => "Checking",
                HubstaffStatus::Unavailable { .. } => "Offline",
            };
            label = if matches!(status, HubstaffStatus::Unavailable { .. }) {
                format!("Hubstaff · {verification}")
            } else {
                format!("Hubstaff · {connection} · {verification}")
            };
            detail.push_str(" Some locally counted time is awaiting Hubstaff evidence. Verification and corrections run automatically; see Details for the interval and result.");
        }
        tooltip(
            container(
                text(label)
                    .size(TEXT_SMALL)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .id(Id::new("hubstaff-status"))
            .padding([SPACE_XS, SPACE_S])
            .center_y(Length::Fill),
            container(
                text(detail)
                    .size(TEXT_SMALL)
                    .width(Length::Fill)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            )
            .padding([SPACE_S, SPACE_M])
            .width(Length::Fixed(300.0)),
            tooltip::Position::Bottom,
        )
        .delay(TOOLTIP_DELAY)
        .into()
    }

    /// One centered column. Details and History are collapsed behind
    /// disclosures so the clock and timers stay on screen.
    pub(crate) fn narrow_layout(&self) -> Element<'_, Message> {
        let mut body = column![
            self.title_bar(),
            self.active_panel(),
            self.timers_panel(),
            self.disclosure(
                "Details",
                self.details_expanded,
                Message::ToggleDetails,
                "details-disclosure"
            ),
        ]
        .spacing(SPACE_XL)
        .width(Length::Fill)
        .max_width(NARROW_MAX_WIDTH);
        if self.details_expanded {
            body = body.push(self.details_panel());
        }
        body = body.push(self.disclosure(
            "History",
            self.history_expanded,
            Message::ToggleHistory,
            "history-disclosure",
        ));
        if self.history_expanded {
            body = body.push(self.history_panel(Length::Fixed(HISTORY_LIST_HEIGHT)));
        }
        scrollable(
            container(body)
                .padding(PAD_PAGE)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .id(Id::new("root-scroll"))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// Two panes: the clock and timers on the left; Details and History
    /// always open on the right, since there is room for them. The right pane
    /// is exactly the window's height: Details takes its natural height and
    /// History fills the rest, scrolling inside. The left pane scrolls on its
    /// own if the window is too short for it.
    pub(crate) fn wide_layout(&self) -> Element<'_, Message> {
        let left = scrollable(
            column![self.title_bar(), self.active_panel(), self.timers_panel()].spacing(SPACE_XL),
        )
        .id(Id::new("root-scroll"))
        .width(Length::FillPortion(5))
        .height(Length::Fill);
        let right = column![
            container(text("Details").size(TEXT_HEADING))
                .height(Length::Fixed(TITLE_BAR_HEIGHT))
                .center_y(Length::Fixed(TITLE_BAR_HEIGHT)),
            self.details_panel(),
            text("History").size(TEXT_HEADING),
            self.history_panel(Length::Fill),
        ]
        .spacing(SPACE_XL)
        .width(Length::FillPortion(6))
        .height(Length::Fill);
        container(
            container(row![left, right].spacing(SPACE_3XL))
                .width(Length::Fill)
                .max_width(WIDE_MAX_WIDTH)
                .height(Length::Fill),
        )
        .id(Id::new("wide-layout"))
        .padding(PAD_PAGE)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .into()
    }

    pub(crate) fn status_banner(&self) -> Element<'_, Message> {
        let Some(message) = self.error_message.as_deref().or(self.notice.as_deref()) else {
            return widgets::nothing();
        };

        // Errors clear on the next successful action; only a notice (shown
        // when there is no error) can be dismissed.
        let dismiss: Element<'_, Message> = if self.error_message.is_none() {
            button("Dismiss")
                .on_press(Message::DismissNotice)
                .height(Length::Fixed(DIALOG_BUTTON_HEIGHT))
                .into()
        } else {
            Space::new().into()
        };
        let content = row![
            text(message)
                .width(Length::Fill)
                .height(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::Word),
            dismiss,
        ]
        .spacing(SPACE_XL)
        .align_y(iced::Alignment::Center);

        opaque(
            container(content)
                .id(Id::new("status-banner"))
                .padding(PAD_PANEL)
                .width(Length::Fill)
                .height(Length::Fixed(STATUS_OVERLAY_HEIGHT))
                .style(container::rounded_box),
        )
    }

    pub(crate) fn disclosure(
        &self,
        label: &'static str,
        expanded: bool,
        message: Message,
        id: &'static str,
    ) -> Element<'_, Message> {
        container(
            button(row![
                text(label),
                Space::new().width(Length::Fill),
                text(if expanded { "▴" } else { "▾" })
            ])
            .on_press(message)
            .style(button::secondary)
            .width(Length::Fill)
            .height(Length::Fixed(DISCLOSURE_HEIGHT)),
        )
        .id(Id::new(id))
        .width(Length::Fill)
        .height(Length::Fixed(DISCLOSURE_HEIGHT))
        .into()
    }
}

fn compact_project(project: Option<&str>) -> String {
    let Some(project) = project else {
        return "No project".to_owned();
    };
    let mut chars = project.chars();
    let compact: String = chars.by_ref().take(15).collect();
    if chars.next().is_some() {
        format!("{compact}…")
    } else {
        compact
    }
}
