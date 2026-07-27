#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod chain;
pub mod error;
pub mod hash;
pub mod import;
pub mod model;
pub mod protocol;
pub mod varint;
pub mod vm;

pub use chain::ChainAdapterV1;
pub use error::{Error, ErrorKind, Result};
pub use hash::{
    candidate_encoding_hash, encoding_hash, hash_hex, parse_hash_hex, result_id, semantic_root,
    task_id, Hash32,
};
pub use import::{import_asset, import_incumbent, ImportedAsset};
pub use model::*;
pub use protocol::*;
pub use vm::*;

pub const SOFTWARE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PROTOCOL_VERSION: u8 = 1;
pub const VM_VERSION: u8 = 1;
pub const COST_MODEL_VERSION: u8 = 1;

pub const SEMANTIC_DOMAIN: &[u8] = b"NICECHUNK:POUW:SEMANTIC:V1\0";
pub const ENCODING_DOMAIN: &[u8] = b"NICECHUNK:POUW:ENCODING:V1\0";
pub const TASK_DOMAIN: &[u8] = b"NICECHUNK:POUW:TASK:V1\0";
pub const RESULT_DOMAIN: &[u8] = b"NICECHUNK:POUW:RESULT:V1\0";
