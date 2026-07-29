use std::{collections::HashMap, path::Path};

const LEGACY_MAGIC: &[u8; 8] = b"VELVTC01";
const PROTOTYPE_MAGIC: &[u8; 8] = b"VELVTP02";
const LEGACY_ARTIFACT_VERSION: &str = "centroids-v1";

/// Validated, versioned category prototype set loaded beside the ONNX model.
pub struct CategoryCentroids {
    taxonomy_version: String,
    artifact_version: String,
    prototypes: HashMap<String, Vec<Vec<f32>>>,
}

impl CategoryCentroids {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, CentroidError> {
        let bytes = std::fs::read(path).map_err(|_| CentroidError::Read)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CentroidError> {
        let mut cursor = Cursor::new(bytes);
        let magic = cursor.take(8)?;
        let has_artifact_version = if magic == LEGACY_MAGIC {
            false
        } else if magic == PROTOTYPE_MAGIC {
            true
        } else {
            return Err(CentroidError::Invalid);
        };
        let taxonomy_version = cursor.string()?;
        let artifact_version = if has_artifact_version {
            cursor.string()?
        } else {
            LEGACY_ARTIFACT_VERSION.to_owned()
        };
        let dimensions = cursor.u32()? as usize;
        let count = cursor.u32()? as usize;
        if taxonomy_version.is_empty()
            || artifact_version.is_empty()
            || dimensions == 0
            || count == 0
        {
            return Err(CentroidError::Invalid);
        }
        let mut prototypes = HashMap::<String, Vec<Vec<f32>>>::new();
        for _ in 0..count {
            let category = cursor.string()?;
            let mut vector = Vec::with_capacity(dimensions);
            for _ in 0..dimensions {
                vector.push(f32::from_le_bytes(
                    cursor
                        .take(4)?
                        .try_into()
                        .map_err(|_| CentroidError::Invalid)?,
                ));
            }
            if category.is_empty() || vector.iter().any(|value| !value.is_finite()) {
                return Err(CentroidError::Invalid);
            }
            let category_prototypes = prototypes.entry(category).or_default();
            // V1 was defined as exactly one centroid per category. V2 records
            // may repeat a category to represent distinct semantic modes.
            if !has_artifact_version && !category_prototypes.is_empty() {
                return Err(CentroidError::Invalid);
            }
            category_prototypes.push(vector);
        }
        if !cursor.remaining().is_empty() {
            return Err(CentroidError::Invalid);
        }
        Ok(Self {
            taxonomy_version,
            artifact_version,
            prototypes,
        })
    }

    pub fn taxonomy_version(&self) -> &str {
        &self.taxonomy_version
    }

    pub fn artifact_version(&self) -> &str {
        &self.artifact_version
    }

    pub fn categories(&self) -> impl Iterator<Item = &str> {
        self.prototypes.keys().map(String::as_str)
    }

    /// Compatibility accessor for legacy callers. V2 callers should preserve
    /// every category mode with [`Self::into_prototypes`].
    pub fn into_vectors(self) -> HashMap<String, Vec<f32>> {
        self.prototypes
            .into_iter()
            .filter_map(|(category, mut prototypes)| {
                prototypes.drain(..).next().map(|vector| (category, vector))
            })
            .collect()
    }

    pub fn into_prototypes(self) -> HashMap<String, Vec<Vec<f32>>> {
        self.prototypes
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], CentroidError> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(CentroidError::Invalid)?;
        let value = &self.bytes[self.position..end];
        self.position = end;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, CentroidError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| CentroidError::Invalid)?,
        ))
    }

    fn string(&mut self) -> Result<String, CentroidError> {
        let length = self.u32()? as usize;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| CentroidError::Invalid)
    }

    fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.position..]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CentroidError {
    #[error("failed to read abstraction centroid file")]
    Read,
    #[error("abstraction centroid file is invalid")]
    Invalid,
}
