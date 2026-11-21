//! The system ink (SPEC §machines.ink): a shell at its worst with 8
//! machines at theirs (`terse_core::worst`), shown tiled, overlapping and
//! as panels.

use std::collections::HashMap;

use terse_core::machine::FrameResult;
use terse_core::system_ink::SYSTEM_INK;
use terse_core::worst::{Layout, combined_image, system_shell};
use terse_core::{Controller, Driver, Event, FrameInput, Image, Machine, ProgramId, System, Who, CONTROLLERS};

/// Runs every machine and hands it the images it asks for.
#[derive(Default)]
struct Host {
    images: HashMap<[u8; 32], Image>,
    wanted: Vec<(u32, [u8; 32])>,
}

impl Driver for Host {
    fn frame(&mut self, _: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        machine.run_frame(&FrameInput { events: &events, controllers, ..Default::default() })
    }
    fn want_image(&mut self, serial: u32, image: [u8; 32]) {
        self.wanted.push((serial, image));
    }
    fn want_program(&mut self, _: u32, _: &ProgramId, _: &[[u8; 32]]) {}
    fn removed(&mut self, _: u32) {}
}

/// The system of `layout` after `frames` frames.
fn system(layout: Layout, frames: u32) -> System {
    let child = Image::parse(&combined_image()).unwrap();
    let shell = Image::parse(&system_shell(layout, 8, 0)).unwrap();
    let mut host = Host::default();
    host.images.insert(child.id, child.clone());
    let mut system = System::new(&shell, true, vec![child.id]);
    let idle = [Controller { connected: false, buttons: 0, left: (0, 0), right: (0, 0) }; CONTROLLERS];
    for _ in 0..frames {
        for (serial, id) in std::mem::take(&mut host.wanted) {
            system.deliver_image(serial, Ok(host.images[&id].clone()));
        }
        system.frame(&[], idle, &mut host);
    }
    assert!(system.stopped().is_none());
    system
}

/// Overlapping pictures of the whole screen: two fit (614,400 of
/// 691,200), the other six are plates; the second's lists get what is
/// left, at its full scale; the shell is never counted.
#[test]
fn overlapping_pictures_beyond_the_ink_are_plates() {
    let s = system(Layout::Overlap, 6).shares();
    assert_eq!(s.shares.len(), 8);
    let plates: Vec<bool> = s.shares.iter().map(|x| x.plate).collect();
    assert_eq!(plates, [false, false, true, true, true, true, true, true]);
    assert!(s.shares.iter().all(|x| x.n == 640 && x.area == 640 * 480));
    assert_eq!(s.shares[0].cap2, (SYSTEM_INK - 2 * 640 * 480).min(230_400));
    assert!(s.used <= SYSTEM_INK && s.used > SYSTEM_INK - 1000, "{}", s.used);
    assert!(s.cut());
}

/// Tiles of a third of the screen's width: every picture is drawn, the
/// lists count a third of their ink, and the later machines' lists are
/// cut where the system ink runs out.
#[test]
fn tiles_share_what_their_pictures_leave() {
    let s = system(Layout::Tiles, 6).shares();
    assert_eq!(s.shares.len(), 8);
    assert!(s.shares.iter().all(|x| !x.plate && x.n == 213 && x.area == 213 * 159));
    let areas: u64 = s.shares.iter().map(|x| x.area).sum();
    assert!(s.used <= SYSTEM_INK && s.used > areas);
    // The first machine's 2D list is drawn whole (rectangles to its cap),
    // counted at its scale; the last ones get nothing.
    assert_eq!(s.shares[0].cap2, 230_400);
    assert!(s.shares[0].ink >= (230_400u64 * 213).div_ceil(640) && !s.shares[0].cut);
    assert!(s.shares[7].cap2 < 640 && s.shares[7].cut, "{:?}", s.shares[7]);
    // The same system counts the same.
    assert_eq!(s, system(Layout::Tiles, 6).shares());
}

/// Panels of the whole screen in the shell's scene count their box on
/// the glass.
#[test]
fn panels_count_their_box_on_the_glass() {
    let sys = system(Layout::Panels, 6);
    let s = sys.shares();
    assert_eq!(s.shares.len(), 8, "{s:?}");
    assert!(s.shares.iter().all(|x| x.panel));
    assert!(!s.shares[0].plate && s.shares[7].plate);
    assert!(s.used <= SYSTEM_INK);
}
