use crate::frames::{Machine, Status};

impl Machine {
    pub fn state(
        &self,
        id: &[u8; 32],
        grants: &[u8],
        terminal: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut state = b"terse-state-1".to_vec();
        state.extend(id);
        state.extend(self.cpu.pc.to_le_bytes());
        for x in &self.cpu.x[1..] {
            state.extend(x.to_le_bytes());
        }
        state.extend(self.frame.to_le_bytes());
        let (stopped, code, fault) = match self.status {
            Status::Running => (0, 0, None),
            Status::Exited(code) => (1, code, None),
            Status::Faulted(fault) => (2, 0, Some(fault)),
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
        state.extend([0; 37]);
        match terminal {
            Some(section) => {
                state.push(1);
                state.extend(section);
            }
            None => state.push(0),
        }
        state.extend([0, 0, 0]);
        state
    }
}
