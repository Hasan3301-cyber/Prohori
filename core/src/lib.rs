//! Prohori core: the deterministic layer.
//!
//! Everything in this crate runs without a model, without a network, and without
//! randomness. `PLAN.md` §1 puts the model behind this layer, not in front of it —
//! the red-flag rules, the first-aid corpus, and the rendering verifier all work with
//! no weights on the device at all, which is exactly what phase P0 ships.
//!
//! Read `docs/CONVENTIONS.md` before changing anything here. The short version: a
//! safety invariant lands with its test in the same commit, unknown is never safe, and
//! there is no `unwrap` in this crate.

// This crate is not published; its documentation is internal reasoning, read with
// `--document-private-items` (see `.github/workflows/ci.yml`). Linking a public
// explanation to the private function that implements it — `normalize` to
// `canonical_spelling`, `search` to `Index::is_evidence` — is the point of the
// explanation, so the warning that those links would break in a public build is noise
// here. `broken_intra_doc_links` stays denied: a link to something that no longer exists
// is still a failure.
#![allow(rustdoc::private_intra_doc_links)]

pub mod audit;
pub mod bundled;
pub mod city_pack;
pub mod confirmation;
pub mod dataset;
pub mod emergency;
pub mod eval;
pub mod fallback;
pub mod guidance;
pub mod inference;
pub mod normalize;
pub mod protocol;
pub mod readability;
pub mod redflag;
pub mod render;
pub mod retrieval;
pub mod routing;
pub mod severity;
pub mod verifier;
