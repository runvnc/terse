use crate::frames::{Machine, Status};

const STATE_TAG: &[u8] = b"terse-state-1";
const EMPTY_SYSTEM: usize = 37;
const ABSENT: u8 = 0;
const PRESENT: u8 = 1;
const RUNNING: u8 = 0;
const EXITED: u8 = 1;
const FAULTED: u8 = 2;

impl Machine {
    pub fn state(
        &self,
        id: &[u8; 32],
        grants: &[u8],
        terminal: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut state = STATE_TAG.to_vec();
        state.extend(id);
        state.extend(self.cpu.pc.to_le_bytes());
        for x in &self.cpu.x[1..] {
            state.extend(x.to_le_bytes());
        }
        state.extend(self.frame.to_le_bytes());
        let (stopped, code, fault) = match self.status {
            Status::Running => (RUNNING, 0, None),
            Status::Exited(code) => (EXITED, code, None),
            Status::Faulted(fault) => (FAULTED, 0, Some(fault)),
        };
        let kind = fault.map_or(0, |f| f.kind as u8);
        state.extend([stopped, code, kind]);
        for value in [
            fault.map_or(0, |f| f.encoding),
            fault.map_or(0, |_| self.cpu.pc),
            fault.map_or(0, |f| f.address),
            self.budget as u32,
            self.cpu.ram.len() as u32,
        ] {
            state.extend(value.to_le_bytes());
        }
        state.extend(&self.cpu.ram);
        state.extend(grants);
        state.extend([0; EMPTY_SYSTEM]);
        match terminal {
            Some(section) => {
                state.push(PRESENT);
                state.extend(section);
            }
            None => state.push(ABSENT),
        }
        state.extend([ABSENT, ABSENT, ABSENT]);
        state
    }
}
