//! Transcript assembly (Phase 1 onwards).
//!
//! Turns engine words into the stored model: applies relabels, honours
//! `finalized` watermarks, builds segments (contiguous same-speaker words, split
//! on gaps over 1.5 s or sentence punctuation), assigns `seq`, and interleaves
//! mic and loopback channels by time. Later: our own word-to-speaker fusion
//! (v1.5) and the live-preview to post-pass speaker matching.
