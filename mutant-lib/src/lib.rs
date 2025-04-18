//! # MutAnt
//!
//! MutAnt is a private, mutable key-value store built on Autonomi network scratchpads, offering resilient, cost-efficient, and async-first storage.
//!
//! ## Why MutAnt?
//! Addressing on-chain storage limitations, MutAnt:
//! - **Splits** large data into scratchpad-sized chunks.
//! - **Resumes** interrupted transfers automatically.
//! - **Recycles** freed pads to reduce costs.
//! - **Caches** index locally for fast lookups and syncs remotely.
//! - **Adapts** to business logic with pluggable backends.
//!
//! ## Key Highlights
//!
//! - **Chunk Management**: Configurable pad sizes with automatic chunking and reassembly.
//! - **Resumption & Retries**: Transparent retry logic and transfer continuation.
//! - **Cost Efficiency**: Reuses freed pads to minimize redundant on-chain writes.
//! - **Flexible Interfaces**: Rust SDK (`mutant-lib`) and CLI tool (`mutant`).
//! - **Async-First**: Built on `tokio` and `async/await`.
//! - **Extensible Architecture**: Modular design allows custom network layers.
//!
//! ## Quickstart
//!
//! Add to `Cargo.toml`:
//! ```toml
//! mutant-lib = "0.4.0"
//! ```
//!
//! ```rust,no_run
//! use mutant_lib::MutAnt;
//! use anyhow::Result;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // Use a dummy private key for doctest purposes.
//!     let key_hex = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".to_string();
//!
//!     let mut ant = MutAnt::init(key_hex).await?;
//!
//!     ant.store("file1".to_string(), b"hello").await?;
//!
//!     let data = ant.fetch("file1").await?;
//!
//!     println!("Fetched: {}", String::from_utf8_lossy(&data));
//!     Ok(())
//! }
//! ```
//!
//! ### Fetching Public Data (without a private key)
//!
//! If you only need to fetch data that was stored publicly (using `store_public`), you can
//! initialize a lightweight `MutAnt` instance without providing a private key:
//!
//! ```rust,no_run
//! use mutant_lib::MutAnt;
//! use autonomi::ScratchpadAddress;
//! use anyhow::Result;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // Initialize for public fetching (defaults to Mainnet)
//!     let public_fetcher = MutAnt::init_public().await?;
//!
//!     // You need the public address of the data (obtained elsewhere)
//!     let public_address = ScratchpadAddress::from_hex("...")?;
//!
//!     // Fetch the public data
//!     let data = public_fetcher.fetch_public(public_address, None).await?;
//!
//!     println!("Fetched public data: {} bytes", data.len());
//!     Ok(())
//! }
//! ```
//!
//! **Note:** An instance created with `init_public` can *only* be used for `fetch_public`.
//! Other operations requiring a private key (like `store`, `fetch` private data, `remove`, etc.)
//! will fail.
//!
//! ## Resources & Support
//!
//! - API docs   : https://docs.rs/mutant_lib
//! - CLI help   : `mutant --help`
//! - Repository : https://github.com/Champii/MutAnt
//! - Issues     : https://github.com/Champii/MutAnt/issues
//!

/// Provides the main API entry point for interacting with MutAnt.
mod api;
/// Handles data structures and serialization/deserialization logic.
mod data;
/// Manages indexing and search functionality for stored data.
mod index;
/// Contains network-related functionalities, including data persistence via scratchpads.
mod network;
/// Manages the lifecycle of pads, including creation, deletion, and updates.
mod pad_lifecycle;

/// Defines custom error types used throughout the `mutant-lib`.
mod internal_error;
/// Defines events and callbacks used for asynchronous operations and progress reporting.
mod internal_events;
/// Contains core data types and configuration structures used by MutAnt.
mod types;

/// Re-export dependency types needed by CLI
pub use crate::api::MutAnt;

pub mod config {
    pub use crate::network::NetworkChoice;
    pub use crate::types::MutAntConfig;
}
pub mod storage {
    pub use crate::types::{KeyDetails, StorageStats};
    pub use autonomi::ScratchpadAddress;
}
pub mod error {
    pub use crate::data::error::DataError;
    pub use crate::index::error::IndexError;
    pub use crate::internal_error::Error;
    pub use crate::pad_lifecycle::error::PadLifecycleError;
}
pub mod events {
    pub use crate::api::{ReserveCallback, ReserveEvent};
    pub use crate::internal_events::{
        GetCallback, GetEvent, InitCallback, InitProgressEvent, PurgeCallback, PurgeEvent,
        PutCallback, PutEvent,
    };
}
