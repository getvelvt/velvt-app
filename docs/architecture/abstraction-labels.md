# Abstraction Labels

The Rust service owns Velvt's on-device abstraction privacy boundary.

## Label Schema `label-v1`

Labels are privacy-safe behavioral descriptions. The standard format is
`<type>:<behavior>`, using lowercase ASCII letters, digits, and underscores.
Examples include `document:edit`, `meeting:active`, `video:passive`, and
`document:inferred`. The fallback sentinel is the single-component label
`unlogged`. It means the event was captured but could not be safely classified;
product copy calls this state **Unclassified**, never “unlogged activity.”

Labels must never contain app names, window titles, filenames, paths, URLs,
contacts, or other raw identifying content. Title semantic abstraction is not
implemented in MVP; `TitleAbstractor` is the V1 extension point.

## Category Taxonomy

`rust-service/resources/abstraction-taxonomy-mvp-1.json` defines the current
`category_taxonomy_version`, category identifiers, and Tier 1 seed mappings.
The current API-expected value is `mvp-1`. Runtime taxonomy files may be
selected with `VELVT_ABSTRACTION_TAXONOMY_PATH`.

When Tier 2 is enabled, its companion centroid binary declares the same
taxonomy version. Startup rejects a version mismatch.

## Classification Quality

Classification status, confidence, and provenance are separate typed fields.
User rules outrank exact seeds, which outrank contextual heuristics, calibrated
embeddings, generic priors, and fallback. Generic browser identity is only a
low-confidence prior. Conflicting cues and embeddings without a sufficient
top-two margin abstain.
