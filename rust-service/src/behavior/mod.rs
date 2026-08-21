//! The behavioural layer: the frozen feature contract, and the retention that
//! bounds the durable substrate it reads.
//!
//! Nothing in here computes a model yet, and nothing in here can fire an
//! intervention. That is deliberate. `03-BEHAVIORAL-ENGINE-SPEC.md` § 1 asks for
//! the feature contract to be published *before* either model exists, so the
//! online detector and the nightly segmenter cannot quietly disagree about what
//! an observation is — and so the not-identifiable list is on the record before
//! there is any pressure to promote a latent variable into a product claim.

pub mod features;
pub mod retention;

pub use retention::OutOfBlockRunRetentionTarget;
