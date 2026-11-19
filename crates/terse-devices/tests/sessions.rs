//! The scripted sessions of terse-cli's shell, holo, open and snapshots
//! tests, on the device bus: compiled here, where the driver programs are,
//! each session also runs with its devices as driver processes (the Rust
//! drivers, on shared memory), in step with the devices compiled in, and
//! after every frame the shell's and every machine's state hash must be the
//! same (`common/twin.rs`). So every golden and hash those tests check holds
//! for the bus too. `sessions_c_socket.rs` runs them again with the C
//! terminal driver on the socket.

#[path = "../../terse-cli/tests/holo.rs"]
mod holo;
#[path = "../../terse-cli/tests/open.rs"]
mod open;
#[path = "../../terse-cli/tests/shell.rs"]
mod shell;
#[path = "../../terse-cli/tests/snapshots.rs"]
mod snapshots;
