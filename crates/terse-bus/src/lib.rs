//! The device bus prototype (DEVICES.md): a small core host that runs one
//! Terse machine and reaches its devices (all but the System device) as
//! separate driver processes, over a Unix socket or shared memory.
//!
//! - `proto`: the messages and the call table;
//! - `transport`: the socket and shared-memory transports;
//! - `bus`: terse-core's `Devices` over driver processes, so that
//!   terse-core's `Machine` (the CPU, the System device, the frame loop and
//!   the state serialization's frame: the same loop `terse run` runs) runs
//!   on the bus;
//! - `inproc`: the devices compiled in, with the content service and page
//!   layout of a host, for comparison;
//! - `compose`: layers composed into a picture;
//! - `record`: the bus's own recordings;
//! - `driver`: the driver's side, for drivers written in Rust.

pub mod bus;
pub mod compose;
pub mod host;
pub mod driver;
pub mod inproc;
pub mod proto;
pub mod record;
pub mod services;
pub mod signals;
pub mod stats;
pub mod transport;
