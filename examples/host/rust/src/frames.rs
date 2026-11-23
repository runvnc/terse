use crate::cpu::{A0, A7, Cpu, Fault, UNKNOWN_CALL};

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

pub const SYSTEM: u32 = 1;
pub const BASE_CHARGE: i64 = 4;
pub const BAD_BUFFER: u32 = 0xffff_fffe;
const EVENT_SIZE: u32 = 4;

const WAIT_FRAME: u32 = 0;
const NEXT_EVENT: u32 = 1;
const FRAME_COUNT: u32 = 2;
const EXIT: u32 = 6;

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
            let a7 = self.cpu.x[A7];
            let (slot, function) = (a7 >> 8, a7 & 0xff);
            let budget = self.budget;
            let (result, charge) = if slot == SYSTEM {
                (self.system(function), BASE_CHARGE)
            } else {
                devices(&mut self.cpu, budget, slot, function)
            };
            self.cpu.x[A0] = result;
            self.budget -= charge;
            if slot == SYSTEM && function == WAIT_FRAME {
                break;
            }
        }
        self.budget = self.budget.min(0);
        self.frame += 1;
    }

    fn system(&mut self, function: u32) -> u32 {
        let a0 = self.cpu.x[A0];
        let event = self.cpu.memory(a0, EVENT_SIZE, true);
        match function {
            WAIT_FRAME => self.frame + 1,
            NEXT_EVENT if event.is_none() => BAD_BUFFER,
            NEXT_EVENT => 0,
            FRAME_COUNT => self.frame,
            EXIT => {
                self.status = Status::Exited(a0 as u8);
                0
            }
            _ => UNKNOWN_CALL,
        }
    }
}
