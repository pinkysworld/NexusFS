#![forbid(unsafe_code)]

/// Embedded admin UI assets.
pub const INDEX_HTML: &[u8] = include_bytes!("../assets/index.html");
pub const APP_JS: &[u8] = include_bytes!("../assets/app.js");
