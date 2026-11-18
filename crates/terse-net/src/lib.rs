//! Host-side networking for the content device (SPEC 9, NETWORK.md).
//!
//! - [`article`]: `terse:` links in articles (NETWORK 4.5).
//! - [`blob`]: blob IDs (BLAKE3-256) and size limits.
//! - [`store`]: the local blob store, a directory of files named by hex ID.
//! - [`identity`]: Ed25519 feed identities and the secret key file (NETWORK 4.1).
//! - [`local`]: finding a relay when none is named.
//! - [`entry`]: feed entries, signing and validation (NETWORK 4.2).
//! - [`feeds`]: stored entries and the subscription list.
//! - [`profile`]: display names, profile documents, nicknames, search
//!   matching (NETWORK 4.6, 4.7).
//! - [`protocol`]: relay message encoding (NETWORK 3, 4.3).
//! - [`relay`]: a poll-driven relay client over a [`relay::Transport`], with a
//!   blocking WebSocket transport.
//! - [`node`]: host behaviour (NETWORK 2, 4.4): store first, then the relay,
//!   with verification; following and publishing feeds.
//! - [`service`]: runs a node on a background thread for the frame loop.
//! - [`timeline`]: the arrival order of entries and the feed records of SPEC 9.5.

pub mod article;
pub mod blob;
pub mod entry;
pub mod feeds;
pub mod identity;
pub mod local;
pub mod node;
pub mod profile;
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
pub use service::{Asked, Consent, ContentService, FeedCommand, Prompt};
pub use store::{BlobStore, default_config_dir, default_data_dir, default_store_dir};
