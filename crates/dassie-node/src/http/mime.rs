//! Media types of the wallet's files: one table for the build script of
//! `dassie-wallet-assets`, which embeds them, and for the dev server, which
//! reads them from disk.

/// The media type of an extension, and whether it compresses.
#[must_use]
pub fn content_type(extension: &str) -> (&'static str, bool) {
    match extension {
        "html" => ("text/html; charset=utf-8", true),
        "js" | "mjs" => ("text/javascript; charset=utf-8", true),
        "css" => ("text/css; charset=utf-8", true),
        "json" | "map" => ("application/json", true),
        "webmanifest" => ("application/manifest+json", true),
        "svg" => ("image/svg+xml", true),
        "txt" => ("text/plain; charset=utf-8", true),
        // instantiateStreaming refuses WebAssembly of any other type.
        "wasm" => ("application/wasm", true),
        "png" => ("image/png", false),
        "ico" => ("image/x-icon", true),
        "woff2" => ("font/woff2", false),
        "woff" => ("font/woff", false),
        _ => ("application/octet-stream", false),
    }
}
