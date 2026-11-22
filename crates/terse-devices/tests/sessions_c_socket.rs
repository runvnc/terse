//! `sessions.rs` again with the C terminal driver (`c/terminal.c`, built
//! with clang) on the socket transport, and the other drivers on the socket
//! too (`common/twin.rs` chooses by this test's name). The C driver is
//! POSIX (`poll`, Unix-domain sockets through the POSIX headers): on
//! Windows this test says it is skipped.

#[cfg(unix)]
#[path = "../../terse-cli/tests/holo.rs"]
mod holo;
#[cfg(unix)]
#[path = "../../terse-cli/tests/open.rs"]
mod open;
#[cfg(unix)]
#[path = "../../terse-cli/tests/shell.rs"]
mod shell;
#[cfg(unix)]
#[path = "../../terse-cli/tests/snapshots.rs"]
mod snapshots;

#[cfg(not(unix))]
#[test]
fn the_c_terminal_driver_is_posix() {
    terse_testkit::skip!("os", "c/terminal.c is a POSIX driver (poll.h, sys/socket.h); it is not built on Windows");
}
