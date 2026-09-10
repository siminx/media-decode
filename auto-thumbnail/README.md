# auto-thumbnail (deprecated)

This crate has been **renamed to [media-decode](https://crates.io/crates/media-decode)**.

Version 0.3.0 is a compatibility shim that re-exports `media_decode`. New projects should depend on:

```toml
[dependencies]
media-decode = { version = "0.3", features = ["full"] }
```
