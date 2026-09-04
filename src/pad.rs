//! The two analog controls the interface language has no word for.
//!
//! `lxb-input` reads every controller on the machine and answers in *actions*
//! — Left, Accept, Menu, Next — which is deliberate: it is what makes a pad, a
//! keyboard and a wheel one interface instead of three, and an application
//! that went behind it would be inventing a second set of controls nobody else
//! on this desktop has. So nearly all of this application's input comes from
//! there and nothing here duplicates it.
//!
//! The triggers are the exception, and only because scanning is the exception.
//! An action is a thing that happened; a trigger is a quantity — *how far* —
//! and there is no honest way to say "sixty percent" in a list of actions.
//! Scanning through a film is the one control here that genuinely wants the
//! analogue: a fixed jump is right for a button and wrong for a thumb resting
//! on a trigger, which is asking to go *faster*, not to go again.
//!
//! So this opens its own reader for exactly that, and answers one number.
//! It maps nothing, decides no cadence and has no opinion about any button:
//! `lxb_toolkit::input` remains the only thing in this application that says
//! what a control *means*. Two readers on one device is not a conflict — a
//! controller is not a Wayland input device and never passes through a
//! compositor, so every program on the machine already reads the same pad at
//! the same time.

use gilrs::{Axis, Button, Gilrs, GilrsBuilder};

/// How far a trigger must be pulled before it is being pulled at all.
///
/// A resting thumb and a worn spring both sit a little off nought, and a film
/// that crept away from where it was left whenever nobody was touching it
/// would be the most annoying possible bug.
const DEAD: f32 = 0.12;

pub struct Pad {
    pads: Option<Gilrs>,
    /// Why there is no reader, if there is none. Said once, at startup.
    trouble: Option<String>,
}

/// One controller, as it is seen — for `--controllers`, which exists because
/// "the controller does nothing" has two completely different causes and no
/// way to tell them apart from the outside.
pub struct Found {
    pub name: String,
    pub mapped: bool,
    pub triggers: bool,
}

impl Pad {
    pub fn new() -> Pad {
        // Force feedback off, as `lxb-input` does. A rumble request nothing
        // answers blocks for thirty seconds, and this reader is opened on the
        // way to the first frame.
        let (pads, trouble) = match GilrsBuilder::new().with_force_feedback(false).build() {
            Ok(pads) => (Some(pads), None),
            Err(err) => (None, Some(err.to_string())),
        };
        Pad { pads, trouble }
    }

    pub fn trouble(&self) -> Option<&str> {
        self.trouble.as_deref()
    }

    /// Pump the queue, once a frame.
    ///
    /// Not for the events — those are `lxb-input`'s business — but because a
    /// gamepad's stored axis values are only brought up to date by draining
    /// them. Without this the triggers read whatever they were when the
    /// device was opened, for ever.
    pub fn settle(&mut self) {
        if let Some(pads) = self.pads.as_mut() {
            while pads.next_event().is_some() {}
        }
    }

    /// How hard the triggers are being pulled, as one number: the right one
    /// less the left one, each from nought to one.
    ///
    /// One number rather than two because they drive one thing between them
    /// and pulling both should do what pulling neither does. Whichever pad is
    /// pulled hardest wins, so it does not matter which of several is in hand.
    pub fn pull(&self) -> f32 {
        let Some(pads) = self.pads.as_ref() else {
            return 0.0;
        };
        let mut most = 0.0_f32;
        for (_, pad) in pads.gamepads() {
            let out = squeeze(&pad, Button::RightTrigger2, Axis::RightZ);
            let back = squeeze(&pad, Button::LeftTrigger2, Axis::LeftZ);
            let pull = out - back;
            if pull.abs() > most.abs() {
                most = pull;
            }
        }
        most
    }

    pub fn found(&self) -> Vec<Found> {
        let Some(pads) = self.pads.as_ref() else {
            return Vec::new();
        };
        pads.gamepads()
            .map(|(_, pad)| Found {
                name: pad.name().to_string(),
                mapped: pad.mapping_source() == gilrs::MappingSource::SdlMappings,
                triggers: pad.button_data(Button::RightTrigger2).is_some()
                    || pad.axis_data(Axis::RightZ).is_some(),
            })
            .collect()
    }
}

/// How far one trigger is pulled.
///
/// Asked for as a button first and an axis second, because that is the order
/// the two ways a driver can present an analogue trigger are worth trying in:
/// most report it as a button carrying a value, and the rest as an axis.
fn squeeze(pad: &gilrs::Gamepad, button: Button, axis: Axis) -> f32 {
    if let Some(data) = pad.button_data(button) {
        let value = data.value();
        if value > DEAD {
            return value.min(1.0);
        }
        // A pad that reports the button at rest still has nothing to say
        // through the axis; fall through only where there was no button.
        if pad.button_data(button).is_some() && pad.axis_data(axis).is_none() {
            return 0.0;
        }
    }
    let Some(data) = pad.axis_data(axis) else {
        return 0.0;
    };
    pulled_to(data.value())
}

/// How far an axis-reported trigger is pulled, from nought to one.
///
/// There are two conventions and a driver may use either: rest at nought and
/// pull to one, or rest at minus one and pull to plus one. A reading below the
/// dead zone can only be the second — the first never goes negative — so it is
/// the one place the two can be told apart, and the second is folded onto the
/// first.
fn pulled_to(value: f32) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    let pulled = if value < -DEAD {
        (value + 1.0) * 0.5
    } else {
        value
    };
    if pulled > DEAD {
        pulled.min(1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_that_found_nothing_is_not_a_failure() {
        // Never panics and never blocks, whatever this machine has plugged in
        // — including nothing at all, which is the ordinary case on a desktop.
        let pad = Pad::new();
        assert_eq!(pad.pull(), 0.0, "a pad nobody is touching pulls nothing");
        // And it can be asked what it found without opening anything.
        let _ = pad.found();
    }

    #[test]
    fn a_trigger_resting_at_nought_pulls_to_one() {
        assert_eq!(pulled_to(0.0), 0.0);
        assert_eq!(pulled_to(1.0), 1.0);
        assert!((pulled_to(0.5) - 0.5).abs() < 0.001);
    }

    /// The other convention: rest at minus one, pull to plus one. Only a
    /// reading below the dead zone can tell the two apart, because a trigger
    /// of the first kind never goes negative at all.
    #[test]
    fn a_trigger_resting_at_minus_one_is_folded_onto_the_same_range() {
        assert_eq!(pulled_to(-1.0), 0.0, "resting is resting");
        assert!((pulled_to(0.0_f32.max(-0.0)) - 0.0).abs() < 0.001);
        // Half way in, on that convention, is nought.
        assert!((pulled_to(-0.5) - 0.25).abs() < 0.001);
    }

    #[test]
    fn a_resting_thumb_is_not_a_pull() {
        assert_eq!(pulled_to(DEAD * 0.5), 0.0);
        assert_eq!(pulled_to(f32::NAN), 0.0);
    }
}
