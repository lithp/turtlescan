use std::borrow::Cow;

use tui::{style::{Color, Style}, text::Span};

const FOCUSED_BORDER: Color = Color::Gray;
const UNFOCUSED_BORDER: Color = Color::DarkGray;

const FOCUSED_SELECTION: Color = Color::LightGreen;
const UNFOCUSED_SELECTION: Color = Color::Green;

pub fn border_color(focused: bool) -> Color {
    match focused {
        true => FOCUSED_BORDER,
        false => UNFOCUSED_BORDER,
    }
}

pub fn selection_color(focused: bool) -> Color {
    match focused {
        true => FOCUSED_SELECTION,
        false => UNFOCUSED_SELECTION,
    }
}

pub fn selected_span<'a, T>(content: T, selected: bool, focused: bool) -> Span<'a>
where
    T: Into<Cow<'a, str>>,
{
    if selected {
        Span::styled(content, Style::default().bg(selection_color(focused)))
    } else {
        Span::raw(content)
    }
}