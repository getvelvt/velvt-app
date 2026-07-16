//! On-device raw-event abstraction and privacy boundary.

mod centroids;
mod engine;
mod key;
mod normalize;
mod onnx;
mod plugin;
mod store;
mod taxonomy;

pub(crate) use engine::override_label_for_category;
pub use engine::{AbstractedEvent, AbstractionEngine, AbstractionEngineBuilder};
pub use key::RawKey;
#[cfg(feature = "onnx")]
pub use onnx::OrtEmbeddingModel;
pub use plugin::{
    ClassificationPlugin, ClassificationResult, ClassificationTier, EmbeddingError,
    EmbeddingMetrics, EmbeddingModel, EmbeddingSimilarityPlugin,
};
pub use store::{AbstractionMappingStore, InMemoryMappingStore, MappingResolution, StoreError};
pub use taxonomy::{SeedApplication, Taxonomy, TaxonomyError, API_EXPECTED_TAXONOMY_VERSION};
pub use velvt_shared_types::{
    ClassificationConfidence, ClassificationSource, ClassificationStatus,
};

use std::borrow::Cow;

/// V1 extension point for title semantic abstraction. MVP intentionally passes titles through.
///
/// V1: replace semantically sensitive title tokens with category-scoped
/// abstract labels using embedding similarity, enabling personalized insight
/// generation without raw title transmission.
pub trait TitleAbstractor: Send + Sync {
    fn abstract_title<'a>(&self, title: &'a str) -> Cow<'a, str>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultTitleAbstractor;

impl TitleAbstractor for DefaultTitleAbstractor {
    fn abstract_title<'a>(&self, title: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(title)
    }
}

pub type NoOpTitleAbstractor = DefaultTitleAbstractor;
pub use centroids::{CategoryCentroids, CentroidError};
