//! On-device raw-event abstraction and privacy boundary.

mod engine;
mod key;
mod plugin;
mod store;
mod taxonomy;

pub use engine::{AbstractedEvent, AbstractionEngine, AbstractionEngineBuilder};
pub use key::RawKey;
pub use plugin::{AbstractionPlugin, PluginMatch};
pub use store::{AbstractionMappingStore, InMemoryMappingStore, StoreError};
pub(crate) use taxonomy::SeedApplication;
pub use taxonomy::{Taxonomy, TaxonomyError};
