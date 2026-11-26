//! The syntax grammars of the app as one syntect set: bat's (via two-face, pure-Rust regex
//! engine) with ours in `grammars/` (C#, PowerShell) in place of bat's of the same language.
//! `build.rs` merges and dumps them, so Markdown's C# fences and a `.cs` file share the very same
//! grammar and the app never reads grammar YAML: that took 100 to 130 ms for C# and held
//! syntect's global scope lock all along, stalling every other grammar's first use.
//!
//! syntect comes from `vendor/syntect` (its README says what differs from the release):
//! fancy-regex 0.20 compiles a grammar into a third of 0.16's memory, and the
//! lazy DFA cache each thread keeps per regex is capped.
//!
//! A crate of its own so the dump loads optimized in debug builds too (the root manifest gives it
//! `opt-level = 2`): the generic deserializer compiled at opt-level 0 took 22 ms instead of 2.

#![forbid(unsafe_code)]

use syntect::parsing::SyntaxSet;

const DUMP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/grammars.packdump"));

/// The grammar set; lines include their newline when parsed with it. Takes about 2 ms.
pub fn syntax_set() -> SyntaxSet {
    syntect::dumps::from_uncompressed_data(DUMP).expect("build.rs dumps a valid set")
}
