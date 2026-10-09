//! shoebox: portable photo library manager. The binary in `main.rs` is a
//! thin CLI over these modules; integration tests use them directly.

pub mod pets;
pub mod ann;
pub mod arrivals;
pub mod backup;
pub mod browse;
pub mod classify;
pub mod clusters;
pub mod db;
pub mod duplicates;
pub mod faces;
pub mod fingerprint;
pub mod fsinfo;
pub mod geo;
pub mod launcher;
pub mod library;
pub mod media;
pub mod multi;
pub mod organize;
pub mod orientation;
pub mod people;
pub mod phash;
pub mod probe;
pub mod recognize;
pub mod report;
pub mod reveal;
pub mod scan;
pub mod screenshots;
pub mod serve;
pub mod tags;
pub mod thumbs;
pub mod verify;
