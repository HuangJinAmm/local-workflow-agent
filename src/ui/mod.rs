// ui module — gated by the `gui` feature.
#![cfg(feature = "gui")]

pub mod app;
pub mod app_view;
pub mod input;
pub mod model;
pub mod provider;
pub mod session;
pub mod settings;
pub mod storage;
pub mod stream;
