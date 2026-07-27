use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{IncumbentFormat, Profile, Semantics};
use crate::{ENCODING_DOMAIN, RESULT_DOMAIN, SEMANTIC_DOMAIN, TASK_DOMAIN};

pub type Hash32 = [u8; 32];

pub fn sha256_domain(domain: &[u8], parts: &[&[u8]]) -> Hash32 {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

pub fn semantic_root(semantics: &Semantics) -> Hash32 {
    let canonical = semantics.canonical_bytes();
    sha256_domain(
        SEMANTIC_DOMAIN,
        &[&[semantics.profile().as_u8()], canonical.as_slice()],
    )
}

pub fn encoding_hash(profile: Profile, format: IncumbentFormat, bytes: &[u8]) -> Hash32 {
    sha256_domain(
        ENCODING_DOMAIN,
        &[&[profile.as_u8(), format.as_u8()], bytes],
    )
}

pub fn candidate_encoding_hash(profile: Profile, bytes: &[u8]) -> Hash32 {
    encoding_hash(profile, IncumbentFormat::PouwVmV1, bytes)
}

pub fn task_id(canonical_task: &[u8]) -> Hash32 {
    sha256_domain(TASK_DOMAIN, &[canonical_task])
}

pub fn result_id(canonical_result: &[u8]) -> Hash32 {
    sha256_domain(RESULT_DOMAIN, &[canonical_result])
}

pub fn hash_hex(hash: &Hash32) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = Vec::with_capacity(64);
    for byte in hash {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 0x0f)]);
    }
    String::from_utf8(output).expect("hex is valid UTF-8")
}

pub fn parse_hash_hex(value: &str) -> Option<Hash32> {
    if value.len() != 64 {
        return None;
    }
    let mut hash = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        hash[index] = (high << 4) | low;
    }
    Some(hash)
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hashes {
    pub semantic_root: Hash32,
    pub encoding_hash: Hash32,
}
