//! Host-side networking for the content device (SPEC §content, NETWORK.md).
//!
//! - [`article`]: `terse:` links in articles (ARTICLE.md), for prefetch.
//! - [`article_parse`]: a reference parser for ARTICLE.md and the article
//!   dump of the test vectors (`vectors/`).
//! - [`vectors`]: the core test vectors that `terse vectors` writes.
//! - [`blob`]: blob IDs (BLAKE3-256) and size limits.
//! - [`builtin`]: the shell and the built-in programs (SPEC §machines).
//! - [`store`]: the local blob store, a directory of files named by hex ID.
//! - [`identity`]: Ed25519 feed identities and the secret key file (NETWORK 4.1).
//! - [`local`]: finding a relay when none is named.
//! - [`entry`]: feed entries, signing and validation (NETWORK 4.2).
//! - [`feeds`]: stored entries and the subscription list.
//! - [`profile`]: display names, profile documents, nicknames, search
//!   matching (NETWORK 4.6, 4.7).
//! - [`protocol`]: relay message encoding (NETWORK 3, 4.3).
//! - [`relay`]: a poll-driven relay client over a [`relay::Transport`], with a
//!   blocking WebSocket transport (`ws://` and `wss://`).
//! - [`tls`]: TLS for `wss://` relays and for a relay given a certificate.
//! - [`node`]: host behaviour (NETWORK 2, 4.4): store first, then the relay,
//!   with verification; following and publishing feeds.
//! - [`records`]: named records, the data programs keep on this host (SPEC §content.records).
//! - [`snapshots`]: snapshots of the machines a shell runs (SPEC §machines.snapshots).
//! - [`service`]: runs a node on a background thread for the frame loop.
//! - [`timeline`]: the arrival order of entries and the feed records of SPEC §content.feeds.

pub mod address;
pub mod article;
pub mod article_parse;
pub mod blob;
pub mod builtin;
pub mod entry;
pub mod feeds;
pub mod highlight;
pub mod driverlist;
pub mod fontlist;
pub mod identity;
pub mod link;
pub mod local;
pub mod manual;
pub mod manual_site;
pub mod feed_site;
pub mod site_render;
pub mod node;
pub mod profile;
pub mod protocol;
pub mod records;
pub mod relay;
pub mod service;
pub mod snapshots;
pub mod store;
pub mod timeline;
pub mod tls;
pub mod trust;
pub mod try_page;
pub mod vectors;

pub use blob::{BlobId, blob_id, parse_hex, to_hex};
pub use entry::{Entry, SignedEntry};
pub use feeds::{FeedStore, Subscriptions};
pub use identity::{FeedKey, Identity};
pub use node::{Article, Delivery, Done, Draft, FeedEvent, Node};
pub use records::{Owner, Records};
pub use service::{Asked, Consent, ContentService, FeedCommand, HeadlessSystem, Prompt};
pub use snapshots::Snapshots;
pub use store::{BlobStore, default_config_dir, default_data_dir, default_store_dir};
