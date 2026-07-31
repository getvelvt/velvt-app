# Abstraction Labels

The Rust service owns Velvt's on-device abstraction privacy boundary.

## Label Schema `label-v1`

On-device labels are behavioral descriptions. The standard format is
`<type>:<behavior>`, using lowercase ASCII letters, digits, and underscores.
Examples include `document:edit`, `meeting:active`, `video:passive`, and
`document:inferred`. The fallback sentinel is the single-component label
`unlogged`. It means the event was captured but could not be safely classified;
product copy calls this state **Unclassified**, never “unlogged activity.”

Labels must never contain window titles, filenames, paths, URLs, contacts, or
other user-provided content. Curated local labels may classify a known
application, but the upload DTO never serializes them: it derives a fixed,
category-scoped cloud abstraction type instead. The server independently
allowlists that vocabulary and maps every other syntactically valid token to
`system:unknown` before storage or logging. Title semantic abstraction is not
implemented in MVP; `TitleAbstractor` is the V1 extension point.

## Category Taxonomy

`rust-service/resources/abstraction-taxonomy-mvp-1.json` defines the current
`category_taxonomy_version`, category identifiers, and Tier 1 seed mappings.
The current API-expected value is `mvp-1`. Runtime taxonomy files may be
selected with `VELVT_ABSTRACTION_TAXONOMY_PATH`.

When Tier 2 is enabled, its companion prototype binary declares both the
canonical taxonomy version and an independently replaceable classifier artifact
version. Startup rejects a taxonomy mismatch. This allows calibrated semantic
prototypes to improve without silently changing category meaning or historical
taxonomy interpretation.

## Classification Quality

Classification status, confidence, and provenance are separate typed fields.
User rules outrank exact seeds, which outrank contextual heuristics, calibrated
embeddings, generic priors, and fallback. Generic browser identity is only a
low-confidence prior. Conflicting cues and embeddings without a sufficient
top-two margin abstain.

The classifier scores an embedding against every prototype and uses the best
prototype score for each category. Explicit user corrections remain bounded,
resettable, exact device-local rules keyed by an irreversible local mapping key;
raw correction context and embeddings are never uploaded.

ONNX artifacts are preferred when approved and packaged. Otherwise every
architecture uses the dependency-free `builtin-hash-v1` token/subword embedder
with bounded input and reviewed built-in prototypes. Classifier artifact usage
is counted in a local-only telemetry table independently of the canonical
taxonomy version.
