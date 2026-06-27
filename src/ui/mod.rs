// ui module — gated by the `gui` feature.
#![cfg(feature = "gui")]

pub mod model;
pub mod provider;
pub mod settings;
pub mod storage;
pub mod stream;
