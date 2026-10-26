//! The JavaScript bindings (wasm-bindgen) of Dassie's core crates, so that
//! the wallet derives exactly what the node derives, and the website's
//! STREAM toy runs the node's STREAM (spec/areas/identity.md#in-the-browser-and-in-scripts).
//!
//! The core crates know nothing of JavaScript: wasm-bindgen and the
//! platform's randomness stay here, at the edge. Each feature is one WASM
//! module of `@dassie/wasm`, built on its own, so a page loads only what it
//! uses: `identity`, and `stream` with ILP's packet codecs.

#![no_std]

extern crate alloc;

#[cfg(feature = "identity")]
mod identity;
#[cfg(feature = "stream")]
mod ilp;
#[cfg(feature = "stream")]
mod stream;
