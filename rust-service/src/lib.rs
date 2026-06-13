//! Velvt local service library.

pub mod abstraction;
#[cfg(feature = "local_analytics")]
pub mod analytics;
pub mod auth;
pub mod config;
pub mod delivery;
pub mod ipc;
pub mod persistence;
pub mod upload;
