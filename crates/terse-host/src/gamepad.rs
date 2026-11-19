//! Game controllers (SPEC 6) through gilrs.
//!
//! Connected gamepads take the lowest free controller number and keep it
//! until they disconnect. Without gamepad support (gilrs could not start)
//! every controller reads as not connected.

use gilrs::{Axis, Button, GamepadId, Gilrs};
use terse_core::{CONTROLLERS, Controller, pad};

pub struct Gamepads {
    gilrs: Option<Gilrs>,
    slots: [Option<GamepadId>; CONTROLLERS],
}

/// SPEC 6 bytes 1 and 2, in bit order.
const BUTTONS: [(Button, u16); 16] = [
    (Button::DPadUp, pad::UP),
    (Button::DPadDown, pad::DOWN),
    (Button::DPadLeft, pad::LEFT),
    (Button::DPadRight, pad::RIGHT),
    (Button::South, pad::A),
    (Button::East, pad::B),
    (Button::West, pad::X),
    (Button::North, pad::Y),
    (Button::LeftTrigger, pad::L1),
    (Button::RightTrigger, pad::R1),
    (Button::LeftTrigger2, pad::L2),
    (Button::RightTrigger2, pad::R2),
    (Button::Select, pad::SELECT),
    (Button::Start, pad::START),
    (Button::LeftThumb, pad::L3),
    (Button::RightThumb, pad::R3),
];

/// A gilrs axis value (−1 to 1) as −127 to 127; `flip` for gilrs's y axes,
/// which are positive upwards.
fn axis(value: f32, flip: bool) -> i8 {
    let v = if flip { -value } else { value };
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

impl Gamepads {
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => Some(g),
            Err(gilrs::Error::NotImplemented(g)) => Some(g),
            Err(e) => {
                eprintln!("terse-host: no game controllers: {e}");
                None
            }
        };
        Gamepads { gilrs, slots: [None; CONTROLLERS] }
    }

    /// The controllers' state now, for latching at the start of a frame.
    pub fn poll(&mut self) -> [Controller; CONTROLLERS] {
        let mut state = [Controller::default(); CONTROLLERS];
        let Some(gilrs) = &mut self.gilrs else { return state };
        while gilrs.next_event().is_some() {}
        for slot in &mut self.slots {
            if slot.and_then(|id| gilrs.connected_gamepad(id)).is_none() {
                *slot = None;
            }
        }
        for (id, _) in gilrs.gamepads() {
            if !self.slots.contains(&Some(id))
                && let Some(free) = self.slots.iter_mut().find(|s| s.is_none())
            {
                *free = Some(id);
            }
        }
        for (slot, out) in self.slots.iter().zip(&mut state) {
            let Some(g) = slot.and_then(|id| gilrs.connected_gamepad(id)) else { continue };
            *out = Controller {
                connected: true,
                buttons: BUTTONS.iter().filter(|(b, _)| g.is_pressed(*b)).fold(0, |bits, (_, bit)| bits | bit),
                left: (axis(g.value(Axis::LeftStickX), false), axis(g.value(Axis::LeftStickY), true)),
                right: (axis(g.value(Axis::RightStickX), false), axis(g.value(Axis::RightStickY), true)),
            };
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes() {
        assert_eq!(axis(1.0, false), 127);
        assert_eq!(axis(-1.0, false), -127);
        assert_eq!(axis(1.0, true), -127, "gilrs up is SPEC up (negative)");
        assert_eq!(axis(-2.0, true), 127);
        assert_eq!(axis(0.0, false), 0);
    }
}
