//! Host-side networking for the content device (SPEC 7, NETWORK.md).
//!
//! - [`blob`]: blob IDs (BLAKE3-256) and size limits.
//! - [`store`]: the local blob store, a directory of files named by hex ID.
//! - [`identity`]: Ed25519 feed identities and the secret key file (NETWORK 4.1).
//! - [`entry`]: feed entries, signing and validation (NETWORK 4.2).
//! - [`feeds`]: stored entries and the subscription list.
//! - [`protocol`]: relay message encoding (NETWORK 3, 4.3).
//! - [`relay`]: a poll-driven relay client over a [`relay::Transport`], with a
//!   blocking WebSocket transport.
//! - [`node`]: host behaviour (NETWORK 2, 4.4): store first, then the relay,
//!   with verification; following and publishing feeds.
//! - [`service`]: runs a node on a background thread for the frame loop.
//! - [`timeline`]: the arrival order of entries and the feed records of SPEC 7.7.

pub mod blob;
pub mod entry;
pub mod feeds;
pub mod identity;
pub mod node;
pub mod protocol;
pub mod relay;
pub mod service;
pub mod store;
pub mod timeline;

pub use blob::{BlobId, blob_id, parse_hex, to_hex};
pub use entry::{Entry, SignedEntry};
pub use feeds::{FeedStore, Subscriptions};
pub use identity::{FeedKey, Identity};
pub use node::{Article, Done, Draft, FeedEvent, Node};
pub use service::{ContentService, FeedCommand};
pub use store::{BlobStore, default_config_dir, default_store_dir};
