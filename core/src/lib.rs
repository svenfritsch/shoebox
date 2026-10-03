//! shoebox: portable photo library manager. The binary in `main.rs` is a
//! thin CLI over these modules; integration tests use them directly.

pub mod classify;
pub mod db;
pub mod fingerprint;
pub mod fsinfo;
pub mod library;
pub mod media;
pub mod probe;
pub mod scan;
pub mod verify;
