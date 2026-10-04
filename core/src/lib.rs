//! shoebox: portable photo library manager. The binary in `main.rs` is a
//! thin CLI over these modules; integration tests use them directly.

pub mod browse;
pub mod classify;
pub mod db;
pub mod duplicates;
pub mod fingerprint;
pub mod fsinfo;
pub mod import;
pub mod library;
pub mod media;
pub mod organize;
pub mod phash;
pub mod probe;
pub mod recognize;
pub mod reveal;
pub mod scan;
pub mod serve;
pub mod thumbs;
pub mod verify;
