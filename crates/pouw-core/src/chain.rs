//! Deliberately transport-agnostic chain integration boundary.
//!
//! PoUW v1 ships no network implementation. A future adapter may fetch and
//! submit canonical binary envelopes, but it must not replace local decoding
//! and verification with an RPC assertion.

use alloc::vec::Vec;

use crate::protocol::NetworkReferenceV1;

/// Optional integration point for a chain, task server, or other transport.
///
/// Implementations live outside `pouw-core`; native and WASM v1 do not provide
/// one. Both methods exchange canonical binary TaskV1/ResultV1 envelopes so a
/// caller can parse and independently verify them with this crate.
pub trait ChainAdapterV1 {
    type Error;
    type Submission;

    fn fetch_task_bytes(
        &self,
        reference: &NetworkReferenceV1,
    ) -> core::result::Result<Vec<u8>, Self::Error>;

    fn submit_result_bytes(
        &self,
        task_bytes: &[u8],
        result_bytes: &[u8],
    ) -> core::result::Result<Self::Submission, Self::Error>;
}
