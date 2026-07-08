use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::{
    COMPOSER_BACKGROUND, COMPOSER_BOTTOM_PADDING, COMPOSER_PREFIX, COMPOSER_RIGHT_PADDING,
    COMPOSER_TOP_PADDING, display_width,
};

pub(crate) fn user_surface_style() -> Style {
    Style::default().bg(COMPOSER_BACKGROUND)
}

pub(crate) fn composer_prompt_width() -> u16 {
    display_width(COMPOSER_PREFIX)
        .min(usize::from(u16::MAX))
        .try_into()
        .unwrap_or(u16::MAX)
}

pub(crate) fn composer_wrap_width(total_width: u16) -> u16 {
    total_width
        .saturating_sub(composer_prompt_width())
        .saturating_sub(COMPOSER_RIGHT_PADDING)
        .max(1)
}

pub(crate) fn composer_text_area_rect(area: Rect) -> Rect {
    let height = area
        .height
        .saturating_sub(COMPOSER_TOP_PADDING)
        .saturating_sub(COMPOSER_BOTTOM_PADDING)
        .max(1);
    Rect::new(
        area.x,
        area.y.saturating_add(COMPOSER_TOP_PADDING),
        area.width.saturating_sub(COMPOSER_RIGHT_PADDING),
        height,
    )
}
