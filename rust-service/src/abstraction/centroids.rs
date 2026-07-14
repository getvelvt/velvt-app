use std::{collections::HashMap, path::Path};

const MAGIC: &[u8; 8] = b"VELVTC01";

/// Validated, versioned category centroid set loaded beside the ONNX model.
pub struct CategoryCentroids {
    taxonomy_version: String,
    vectors: HashMap<String, Vec<f32>>,
}

impl CategoryCentroids {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, CentroidError> {
        let bytes = std::fs::read(path).map_err(|_| CentroidError::Read)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CentroidError> {
        let mut cursor = Cursor::new(bytes);
        if cursor.take(8)? != MAGIC {
            return Err(CentroidError::Invalid);
        }
        let taxonomy_version = cursor.string()?;
        let dimensions = cursor.u32()? as usize;
        let count = cursor.u32()? as usize;
        if taxonomy_version.is_empty() || dimensions == 0 || count == 0 {
            return Err(CentroidError::Invalid);
        }
        let mut vectors = HashMap::with_capacity(count);
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
            if category.is_empty()
                || vector.iter().any(|value| !value.is_finite())
                || vectors.insert(category, vector).is_some()
            {
                return Err(CentroidError::Invalid);
            }
        }
        if !cursor.remaining().is_empty() {
            return Err(CentroidError::Invalid);
        }
        Ok(Self {
            taxonomy_version,
            vectors,
        })
    }

    pub fn taxonomy_version(&self) -> &str {
        &self.taxonomy_version
    }

    pub fn categories(&self) -> impl Iterator<Item = &str> {
        self.vectors.keys().map(String::as_str)
    }

    pub fn into_vectors(self) -> HashMap<String, Vec<f32>> {
        self.vectors
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
