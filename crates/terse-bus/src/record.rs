//! The bus's recordings (DEVICES.md 8): the host's inputs and each
//! driver's opaque input record, per frame, with the state hash after it.

use terse_core::{CONTROLLERS, Controller, Event};

use crate::proto::Reader;

pub const MAGIC: &[u8; 4] = b"TBRC";

/// One frame's inputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameRecord {
    pub frame: u32,
    pub time: u32,
    pub controllers: [[u8; 8]; CONTROLLERS],
    pub events: Vec<Event>,
    pub random: Vec<u8>,
    pub drivers: Vec<(u8, Vec<u8>)>,
    pub hash: [u8; 32],
}

pub fn header(image_id: &[u8; 32]) -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    v.push(0);
    v.extend_from_slice(image_id);
    v
}

impl FrameRecord {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.frame.to_le_bytes());
        out.extend_from_slice(&self.time.to_le_bytes());
        for c in &self.controllers {
            out.extend_from_slice(c);
        }
        out.extend_from_slice(&(self.events.len() as u32).to_le_bytes());
        for e in &self.events {
            out.extend_from_slice(&[e.kind, e.a, e.b, e.c]);
        }
        out.extend_from_slice(&(self.random.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.random);
        out.extend_from_slice(&(self.drivers.len() as u32).to_le_bytes());
        for (slot, b) in &self.drivers {
            out.push(*slot);
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
        out.extend_from_slice(&self.hash);
    }

    pub fn controllers(&self) -> [Controller; CONTROLLERS] {
        self.controllers.map(|b| {
            if b[0] == 0 {
                return Controller::default();
            }
            Controller { connected: true, buttons: u16::from_le_bytes([b[1], b[2]]), left: (b[3] as i8, b[4] as i8), right: (b[5] as i8, b[6] as i8) }
        })
    }
}

/// Parses a recording: the image ID and the frames.
pub fn parse(bytes: &[u8]) -> Result<([u8; 32], Vec<FrameRecord>), String> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != MAGIC || r.u8()? != 0 {
        return Err("not a terse-bus recording".into());
    }
    let id: [u8; 32] = r.take(32)?.try_into().unwrap();
    let mut frames = Vec::new();
    while !r.clone().rest().is_empty() {
        let mut f = FrameRecord { frame: r.u32()?, time: r.u32()?, ..Default::default() };
        for c in &mut f.controllers {
            *c = r.take(8)?.try_into().unwrap();
        }
        let e = r.u32()?;
        for _ in 0..e {
            let b = r.take(4)?;
            f.events.push(Event { kind: b[0], a: b[1], b: b[2], c: b[3] });
        }
        f.random = r.blob()?.to_vec();
        let d = r.u32()?;
        for _ in 0..d {
            let slot = r.u8()?;
            f.drivers.push((slot, r.blob()?.to_vec()));
        }
        f.hash = r.take(32)?.try_into().unwrap();
        frames.push(f);
    }
    Ok((id, frames))
}
