//! The device bus prototype (DEVICES.md): a small core host that runs one
//! Terse machine and reaches its devices (all but the System device) as
//! separate driver processes, over a Unix socket or shared memory.
//!
//! - `proto`: the messages and the call table;
//! - `transport`: the socket and shared-memory transports;
//! - `core`: the machine, the System device, the frame loop, the state
//!   serialization's frame, and the [`core::Devices`] trait;
//! - `bus`: [`core::Devices`] over driver processes;
//! - `inproc`: [`core::Devices`] with the devices compiled in, for
//!   comparison;
//! - `compose`: layers composed into a picture;
//! - `record`: the bus's own recordings;
//! - `driver`: the driver's side, for drivers written in Rust.

pub mod bus;
pub mod compose;
pub mod core;
pub mod driver;
pub mod inproc;
pub mod proto;
pub mod record;
pub mod stats;
pub mod transport;
