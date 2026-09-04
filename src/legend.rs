//! What the buttons do, drawn rather than spelled out.
//!
//! The shell writes this row in the corner of its start screen and so does
//! every application beside it: a word, and a picture of the button that does
//! it. Drawn rather than lettered because the same act is South on a pad and
//! Enter on a keyboard, and there is no wording that names both without naming
//! neither.
//!
//! Three rules come with it, all the shell's:
//!
//! * **A legend naming a button that does nothing is worse than naming none.**
//!   Every hint is asked of the same state that answers the press.
//! * **Nothing about moving.** The arrows are the one thing a page does not
//!   have to explain — and in the viewer they are the one thing that changes
//!   meaning, which the picture itself says better than a word could.
//! * **A pair is a control.** Each is a target a pointer can land on, and a
//!   click on one is a press of the button it pictures, which is how a mouse
//!   gets back out of a page whose Back is only ever drawn here.

use lxb_render::{Align, Ui};
use lxb_toolkit::{input::Action, palette::Role, settings::IconStyle, typography::Text};

#[derive(Debug, Clone, Copy)]
pub struct Hint {
    pub label: &'static str,
    pub on: Button,
}

/// The buttons a legend ever names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Accept,
    Options,
    Back,
    /// Start, which has no key on a keyboard that anything shipped here
    /// pictures — so it is named only while a pad is in hand, and lives in the
    /// Options menu the rest of the time.
    Start,
}

impl Button {
    pub fn action(self) -> Action {
        match self {
            Button::Accept => Action::Accept,
            Button::Options => Action::Menu,
            Button::Back => Action::Back,
            Button::Start => Action::Submit,
        }
    }

    /// The picture of this button, given what the user's hands are on.
    ///
    /// `None` where this build has no honest picture of it, which is a reason
    /// to leave the pair out rather than to draw the wrong button.
    pub fn glyph(self, pad: bool) -> Option<&'static str> {
        Some(match (self, pad) {
            (Button::Accept, true) => "pad-south",
            (Button::Accept, false) => "key-enter",
            (Button::Options, true) => "pad-north",
            (Button::Options, false) => "mouse-right",
            (Button::Back, true) => "pad-east",
            (Button::Back, false) => "key-escape",
            (Button::Start, true) => "pad-start",
            (Button::Start, false) => return None,
        })
    }
}

pub const fn hint(label: &'static str, on: Button) -> Hint {
    Hint { label, on }
}

const GLYPH: f32 = 28.0;
const GAP: f32 = 7.0;
const STEP: f32 = 22.0;

/// Where a pointer can land on this row. Clear of everything the pages number.
pub const SPOT: u32 = 0x8000;

/// Lay a legend out from `right` leftwards, centred on `middle`, and answer
/// where its left-hand end came out.
///
/// Right to left because the words are different lengths and the row is hung
/// off the margin: built this way the pair nearest the corner lands exactly on
/// it, whatever the words turn out to measure.
pub fn row(
    ui: &mut Ui,
    right: f32,
    middle: f32,
    hints: &[Hint],
    pad: bool,
    icons: IconStyle,
) -> f32 {
    let glyph = ui.s(GLYPH);
    let gap = ui.s(GAP);
    let step = ui.s(STEP);
    let label = ui.line(Text::Caption);

    let mut at = right;
    for (index, hint) in hints.iter().enumerate().rev() {
        let Some(mark) = hint.on.glyph(pad) else {
            continue;
        };
        let word = ui.measure(Text::Caption, hint.label);
        let ends = at - glyph - gap;
        ui.spot(
            SPOT + index as u32,
            [
                ends - word - gap * 0.5,
                middle - glyph * 0.5 - gap * 0.5,
                word + gap * 2.0 + glyph,
                glyph + gap,
            ],
        );
        ui.icon_tinted(
            [at - glyph, middle - glyph * 0.5, glyph, glyph],
            mark,
            icons,
            Role::Text,
            0.85,
        );
        ui.label(
            [ends - word, middle - label * 0.5, word, label],
            Text::Caption,
            hint.label,
            Role::TextSoft,
            Align::Right,
        );
        at = ends - word - step;
    }
    at
}

/// Which button a click landed on, if it landed on the row at all.
///
/// Asked of the same hints the row was drawn from, so a pair that was not
/// drawn — Start, on a keyboard — cannot be pressed.
pub fn pressed(id: u32, hints: &[Hint], pad: bool) -> Option<Button> {
    for (index, hint) in hints.iter().enumerate() {
        if hint.on.glyph(pad).is_some() && id == SPOT + index as u32 {
            return Some(hint.on);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_pictured_is_a_mark_the_toolkit_has() {
        for button in [Button::Accept, Button::Options, Button::Back, Button::Start] {
            for pad in [true, false] {
                if let Some(glyph) = button.glyph(pad) {
                    assert!(
                        lxb_toolkit::assets::glyph(glyph).is_some(),
                        "a legend asks for a mark that is not in the toolkit: {glyph}"
                    );
                }
            }
        }
    }

    #[test]
    fn start_is_named_only_where_it_can_be_pictured() {
        assert!(Button::Start.glyph(true).is_some());
        assert!(Button::Start.glyph(false).is_none());
    }

    #[test]
    fn a_pair_that_was_not_drawn_cannot_be_pressed() {
        let hints = [
            hint("Play", Button::Accept),
            hint("Play the folder", Button::Start),
        ];
        assert_eq!(pressed(SPOT + 1, &hints, true), Some(Button::Start));
        assert_eq!(pressed(SPOT + 1, &hints, false), None);
    }
}
