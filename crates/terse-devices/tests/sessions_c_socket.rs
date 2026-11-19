//! `sessions.rs` again with the C terminal driver (`c/terminal.c`, built
//! with clang) on the socket transport, and the other drivers on the socket
//! too (`common/twin.rs` chooses by this test's name).

#[path = "../../terse-cli/tests/holo.rs"]
mod holo;
#[path = "../../terse-cli/tests/open.rs"]
mod open;
#[path = "../../terse-cli/tests/shell.rs"]
mod shell;
#[path = "../../terse-cli/tests/snapshots.rs"]
mod snapshots;
