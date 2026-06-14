use velvt_service::abstraction::CategoryCentroids;

fn string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

#[test]
fn centroid_file_is_versioned_and_loads_vectors() {
    let mut bytes = b"VELVTC01".to_vec();
    string(&mut bytes, "mvp-1");
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    string(&mut bytes, "FOCUS_WORK");
    bytes.extend_from_slice(&1.0_f32.to_le_bytes());
    bytes.extend_from_slice(&0.0_f32.to_le_bytes());

    let centroids = CategoryCentroids::from_bytes(&bytes).unwrap();
    assert_eq!(centroids.taxonomy_version(), "mvp-1");
    assert_eq!(
        centroids.into_vectors().get("FOCUS_WORK"),
        Some(&vec![1.0, 0.0])
    );
}

#[test]
fn invalid_centroid_file_is_rejected() {
    assert!(CategoryCentroids::from_bytes(b"not-a-centroid-file").is_err());
}
