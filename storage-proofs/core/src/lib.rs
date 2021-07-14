#![deny(clippy::all, clippy::perf, clippy::correctness, rust_2018_idioms)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::type_repetition_in_bounds)]
#![warn(clippy::unwrap_used)]

pub use self::data::Data;

#[macro_use]
pub mod test_helper;

pub mod cache_key;
pub mod compound_proof;
pub mod crypto;
pub mod data;
pub mod drgraph;
pub mod error;
pub mod gadgets;
pub mod hasher;
pub mod measurements;
pub mod merkle;
pub mod multi_proof;
pub mod parameter_cache;
pub mod partitions;
pub mod pieces;
pub mod por;
pub mod proof;
pub mod sector;
pub mod settings;
pub mod util;

#[cfg(test)]
pub(crate) const TEST_SEED: [u8; 16] = [
    0x59, 0x62, 0xbe, 0x5d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06, 0xbc, 0xe5,
];
