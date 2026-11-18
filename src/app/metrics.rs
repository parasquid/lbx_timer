//! Layout dimensions, type sizes, and spacing, in logical pixels.

use std::time::Duration;

/// How often the running timer is advanced and checkpointed.
pub(crate) const TICK_INTERVAL: Duration = Duration::from_millis(500);
/// Hover labels wait this long, so moving across buttons does not flash them.
pub(crate) const TOOLTIP_DELAY: Duration = Duration::from_millis(500);
/// Idle stretches shorter than this are not shown in the session timeline.
pub(crate) const TIMELINE_MIN_GAP_MS: i64 = 1_000;

// Window and layout.
/// Default window. Taller than the single-timer layout so the linked timer
/// list fits below the clock without scrolling the controls away.
pub(crate) const WINDOW_SIZE: (f32, f32) = (480.0, 720.0);
/// Widest the single-column layout (or the wide layout's left pane) grows.
pub(crate) const NARROW_MAX_WIDTH: f32 = 640.0;
/// From this window width on, Details and History move into a second pane.
pub(crate) const WIDE_BREAKPOINT: f32 = 960.0;
/// Widest the two-pane layout grows; wider windows center it.
pub(crate) const WIDE_MAX_WIDTH: f32 = 1280.0;

// Panels.
/// The clock card; fixed so state changes never move its controls.
pub(crate) const ACTIVE_PANEL_HEIGHT: f32 = 230.0;
pub(crate) const DISCLOSURE_HEIGHT: f32 = 44.0;
pub(crate) const STATUS_OVERLAY_HEIGHT: f32 = 72.0;
/// History's list in the single-column layout.
pub(crate) const HISTORY_LIST_HEIGHT: f32 = 320.0;
pub(crate) const EVENT_LOG_HEIGHT: f32 = 180.0;

// Timer list.
pub(crate) const TIMER_ROW_HEIGHT: f32 = 40.0;
pub(crate) const TIMER_ROW_SPACING: f32 = 4.0;
/// Rows shown before the timer list scrolls.
pub(crate) const VISIBLE_TIMER_ROWS: usize = 4;
/// The play/pause button is square.
pub(crate) const TIMER_TOGGLE_WIDTH: f32 = TIMER_ROW_HEIGHT;
pub(crate) const TIMER_ICON_SIZE: f32 = 16.0;
pub(crate) const TIMER_REMOVE_WIDTH: f32 = 32.0;

// History.
pub(crate) const HISTORY_ROW_HEIGHT: f32 = 56.0;
pub(crate) const HISTORY_TASK_ROW_HEIGHT: f32 = 36.0;

// Type scale.
/// Secondary line under a value (e.g. a running total).
pub(crate) const TEXT_CAPTION: f32 = 11.0;
/// Timestamps.
pub(crate) const TEXT_META: f32 = 12.0;
/// Timeline rows, links, and hover labels.
pub(crate) const TEXT_SMALL: f32 = 13.0;
/// Body text in panels and lists.
pub(crate) const TEXT_BODY: f32 = 14.0;
/// Labels on row and dialog buttons.
pub(crate) const TEXT_BUTTON: f32 = 15.0;
pub(crate) const TEXT_DEFAULT: f32 = 16.0;
pub(crate) const TEXT_HEADING: f32 = 18.0;
/// A timer's name as a title (clock card, read-only Details).
pub(crate) const TEXT_TITLE: f32 = 20.0;
pub(crate) const TEXT_DIALOG_TITLE: f32 = 22.0;
pub(crate) const TEXT_APP_TITLE: f32 = 25.0;
pub(crate) const TEXT_ERROR_TITLE: f32 = 30.0;
pub(crate) const TEXT_CLOCK: f32 = 64.0;
/// The × on a removable timer row.
pub(crate) const TEXT_REMOVE_GLYPH: f32 = 18.0;

// Spacing scale.
pub(crate) const SPACE_2XS: f32 = 2.0;
pub(crate) const SPACE_XS: f32 = 4.0;
pub(crate) const SPACE_S: f32 = 6.0;
pub(crate) const SPACE_M: f32 = 8.0;
pub(crate) const SPACE_L: f32 = 10.0;
pub(crate) const SPACE_XL: f32 = 12.0;
pub(crate) const SPACE_2XL: f32 = 16.0;
/// Between the two panes of the wide layout.
pub(crate) const SPACE_3XL: f32 = 24.0;

// Padding.
/// Around the page and inside cards and panels.
pub(crate) const PAD_PAGE: f32 = 20.0;
/// Inside the timer list panel and the status banner.
pub(crate) const PAD_PANEL: f32 = 12.0;
pub(crate) const PAD_INPUT: f32 = 10.0;
pub(crate) const PAD_ERROR_PAGE: f32 = 36.0;

// Component dimensions.
pub(crate) const TITLE_BAR_HEIGHT: f32 = 36.0;
/// The clock card's main buttons.
pub(crate) const BUTTON_HEIGHT: f32 = 42.0;
/// Buttons in dialogs and the status banner.
pub(crate) const DIALOG_BUTTON_HEIGHT: f32 = 40.0;
/// "+ Add timer" and "Retry saving".
pub(crate) const SMALL_BUTTON_HEIGHT: f32 = 34.0;
pub(crate) const DIALOG_WIDTH: f32 = 340.0;
pub(crate) const REPORT_WIDTH: f32 = 640.0;
pub(crate) const REPORT_MAX_HEIGHT: f32 = 680.0;
pub(crate) const CLOCK_STATE_HEIGHT: f32 = 24.0;
pub(crate) const CLOCK_HEIGHT: f32 = 80.0;
pub(crate) const CLOCK_TITLE_HEIGHT: f32 = 30.0;
/// Current-session total plus the reserved cumulative-total caption.
pub(crate) const TIMERS_HEADER_HEIGHT: f32 = 40.0;
pub(crate) const SESSION_TOTAL_CAPTION_HEIGHT: f32 = 16.0;
/// A `00:00:00` column at default text size.
pub(crate) const TIME_COLUMN_WIDTH: f32 = 80.0;
/// A `00:00:00` column at body text size.
pub(crate) const TIME_COLUMN_SMALL_WIDTH: f32 = 72.0;
/// A time with the "… total" caption under it.
pub(crate) const TIME_WITH_TOTAL_WIDTH: f32 = 84.0;
/// Percent share in a history row.
pub(crate) const SHARE_COLUMN_WIDTH: f32 = 40.0;
/// Short columns: timeline start times and the history count.
pub(crate) const COMPACT_COLUMN_WIDTH: f32 = 48.0;
pub(crate) const SHARE_BAR_HEIGHT: f32 = 8.0;
pub(crate) const HISTORY_TITLE_HEIGHT: f32 = 22.0;
pub(crate) const HISTORY_LINE_HEIGHT: f32 = 20.0;
/// Two lines (title and totals) inside a history session row.
pub(crate) const HISTORY_ROW_CONTENT_HEIGHT: f32 = 44.0;
pub(crate) const SWATCH_SIZE: f32 = 10.0;
pub(crate) const SWATCH_RADIUS: f32 = 3.0;
pub(crate) const TOOLTIP_RADIUS: f32 = 4.0;
pub(crate) const BORDER_WIDTH: f32 = 1.0;
