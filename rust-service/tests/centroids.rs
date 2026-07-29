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
    assert_eq!(centroids.artifact_version(), "centroids-v1");
    assert_eq!(
        centroids.into_vectors().get("FOCUS_WORK"),
        Some(&vec![1.0, 0.0])
    );
}

#[test]
fn prototype_file_preserves_multiple_modes_per_category() {
    let mut bytes = b"VELVTP02".to_vec();
    string(&mut bytes, "mvp-1");
    string(&mut bytes, "classifier-2026-07-29");
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&3_u32.to_le_bytes());
    for (category, vector) in [
        ("FOCUS_WORK", [1.0_f32, 0.0_f32]),
        ("FOCUS_WORK", [0.0_f32, 1.0_f32]),
        ("REFERENCE", [0.5_f32, 0.5_f32]),
    ] {
        string(&mut bytes, category);
        for value in vector {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }

    let prototypes = CategoryCentroids::from_bytes(&bytes).unwrap();
    assert_eq!(prototypes.taxonomy_version(), "mvp-1");
    assert_eq!(prototypes.artifact_version(), "classifier-2026-07-29");
    let prototypes = prototypes.into_prototypes();
    assert_eq!(prototypes["FOCUS_WORK"].len(), 2);
    assert_eq!(prototypes["REFERENCE"].len(), 1);
}

#[test]
fn prototype_file_requires_an_artifact_version() {
    let mut bytes = b"VELVTP02".to_vec();
    string(&mut bytes, "mvp-1");
    string(&mut bytes, "");
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    string(&mut bytes, "FOCUS_WORK");
    bytes.extend_from_slice(&1.0_f32.to_le_bytes());
    bytes.extend_from_slice(&0.0_f32.to_le_bytes());

    assert!(CategoryCentroids::from_bytes(&bytes).is_err());
}

#[test]
fn invalid_centroid_file_is_rejected() {
    assert!(CategoryCentroids::from_bytes(b"not-a-centroid-file").is_err());
}
