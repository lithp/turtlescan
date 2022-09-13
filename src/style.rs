use tui::style::Color;

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
