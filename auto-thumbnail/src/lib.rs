//! `auto-thumbnail` 已重命名为 [`media-decode`]；本 crate 仅作 crates.io 兼容转发。
//!
//! [`media-decode`]: https://crates.io/crates/media-decode

#![deprecated(
    since = "0.3.0",
    note = "Crate renamed to media-decode; use the media_decode crate instead."
)]

pub use media_decode::*;
