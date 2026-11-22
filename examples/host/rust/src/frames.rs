use crate::cpu::{Cpu, Fault};

pub const FRAME_BUDGET: i64 = 125_000;

pub enum Status {
    Running,
    Exited(u8),
    Faulted(Fault),
}

pub struct Machine {
    pub cpu: Cpu,
    pub budget: i64,
    pub frame: u32,
    pub status: Status,
}

impl Machine {
    pub fn new(cpu: Cpu) -> Machine {
        Machine {
            cpu,
            budget: 0,
            frame: 0,
            status: Status::Running,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.status, Status::Running)
    }

    pub fn run_frame<D>(&mut self, mut devices: D)
    where
        D: FnMut(&mut Cpu, i64, u32, u32) -> (u32, i64),
    {
        self.budget += FRAME_BUDGET;
        while self.budget > 0 && self.running() {
            let (cost, ecall) = match self.cpu.step() {
                Ok(step) => step,
                Err(fault) => {
                    self.status = Status::Faulted(fault);
                    break;
                }
            };
            self.budget -= cost;
            if !ecall {
                continue;
            }
            let a7 = self.cpu.x[17];
            let (slot, function) = (a7 >> 8, a7 & 0xff);
            let budget = self.budget;
            let (result, charge) = if slot == 1 {
                (self.system(function), 4)
            } else {
                devices(&mut self.cpu, budget, slot, function)
            };
            self.cpu.x[10] = result;
            self.budget -= charge;
            if slot == 1 && function == 0 {
                break;
            }
        }
        self.budget = self.budget.min(0);
        self.frame += 1;
    }

    fn system(&mut self, function: u32) -> u32 {
        let a0 = self.cpu.x[10];
        match function {
            0 => self.frame + 1,
            1 if self.cpu.memory(a0, 4, true).is_none() => {
                0xffff_fffe
            }
            1 => 0,
            2 => self.frame,
            6 => {
                self.status = Status::Exited(a0 as u8);
                0
            }
            _ => u32::MAX,
        }
    }
}
