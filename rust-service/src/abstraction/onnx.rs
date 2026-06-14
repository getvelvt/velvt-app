//! Optional ONNX Runtime adapter for a quantized `all-MiniLM-L6-v2` model.
//!
//! The intended artifact is the Apache-2.0 licensed sentence-transformers
//! `all-MiniLM-L6-v2` model exported to ONNX and INT8-quantized below 50 MB.

#[cfg(feature = "onnx")]
mod enabled {
    use std::{path::Path, sync::Mutex};

    use ort::session::Session;
    use tokenizers::Tokenizer;

    use super::super::{EmbeddingError, EmbeddingModel};

    pub struct OrtEmbeddingModel {
        session: Mutex<Session>,
        tokenizer: Tokenizer,
    }

    impl OrtEmbeddingModel {
        pub fn load(model_path: &Path) -> Result<Self, EmbeddingError> {
            let model_size = std::fs::metadata(model_path)
                .map_err(|_| EmbeddingError::Unavailable)?
                .len();
            if model_size > 50 * 1024 * 1024 {
                return Err(EmbeddingError::Unavailable);
            }
            let tokenizer_path = model_path
                .parent()
                .ok_or(EmbeddingError::Unavailable)?
                .join("tokenizer.json");
            let mut tokenizer =
                Tokenizer::from_file(tokenizer_path).map_err(|_| EmbeddingError::Unavailable)?;
            tokenizer
                .with_truncation(Some(tokenizers::TruncationParams {
                    max_length: 256,
                    ..Default::default()
                }))
                .map_err(|_| EmbeddingError::Unavailable)?;
            let session = Session::builder()
                .and_then(|mut builder| builder.commit_from_file(model_path))
                .map_err(|_| EmbeddingError::Unavailable)?;
            Ok(Self {
                session: Mutex::new(session),
                tokenizer,
            })
        }
    }

    impl EmbeddingModel for OrtEmbeddingModel {
        fn embed(&self, input: &str) -> Result<Vec<f32>, EmbeddingError> {
            let encoding = self
                .tokenizer
                .encode(input, true)
                .map_err(|_| EmbeddingError::Unavailable)?;
            let input_ids: Vec<i64> = encoding
                .get_ids()
                .iter()
                .map(|value| i64::from(*value))
                .collect();
            let attention_mask: Vec<i64> = encoding
                .get_attention_mask()
                .iter()
                .map(|value| i64::from(*value))
                .collect();
            let token_type_ids: Vec<i64> = vec![0; input_ids.len()];
            let shape = [1_usize, input_ids.len()];
            let mut session = self
                .session
                .lock()
                .map_err(|_| EmbeddingError::Unavailable)?;
            let outputs = session
                .run(ort::inputs![
                    "input_ids" => ort::value::Tensor::from_array((shape, input_ids)).map_err(|_| EmbeddingError::Unavailable)?,
                    "attention_mask" => ort::value::Tensor::from_array((shape, attention_mask.clone())).map_err(|_| EmbeddingError::Unavailable)?,
                    "token_type_ids" => ort::value::Tensor::from_array((shape, token_type_ids)).map_err(|_| EmbeddingError::Unavailable)?
                ])
                .map_err(|_| EmbeddingError::Unavailable)?;
            let (_, values) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|_| EmbeddingError::Unavailable)?;
            mean_pool(values, &attention_mask)
        }
    }

    fn mean_pool(values: &[f32], attention_mask: &[i64]) -> Result<Vec<f32>, EmbeddingError> {
        if attention_mask.is_empty() || !values.len().is_multiple_of(attention_mask.len()) {
            return Err(EmbeddingError::Unavailable);
        }
        let dimensions = values.len() / attention_mask.len();
        if dimensions == 0 {
            return Err(EmbeddingError::Unavailable);
        }
        let mut embedding = vec![0.0_f32; dimensions];
        let mut token_count = 0.0_f32;
        for (token, mask) in values.chunks_exact(dimensions).zip(attention_mask) {
            if *mask == 0 {
                continue;
            }
            token_count += 1.0;
            for (output, value) in embedding.iter_mut().zip(token) {
                *output += *value;
            }
        }
        if token_count == 0.0 {
            return Err(EmbeddingError::Unavailable);
        }
        for value in &mut embedding {
            *value /= token_count;
        }
        let norm = embedding
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        if norm == 0.0 {
            return Err(EmbeddingError::Unavailable);
        }
        for value in &mut embedding {
            *value /= norm;
        }
        Ok(embedding)
    }
}

#[cfg(feature = "onnx")]
pub use enabled::OrtEmbeddingModel;
