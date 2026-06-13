//! Velvt local processing service entry point.
//!
//! The service owns local processing and the final privacy boundary. It does
//! not render UI or request macOS permissions.

pub mod abstraction;
#[cfg(feature = "local_analytics")]
pub mod analytics;
pub mod auth;
pub mod config;
pub mod delivery;
pub mod ipc;
pub mod persistence;
pub mod upload;

#[tokio::main]
async fn main() {
    todo!("initialize and run the Velvt local service")
}
