#![allow(dead_code)]
use super::{encode_frame, decode_frame, WriteSet, MAX_KEY_BYTES, MAX_VALUE_BYTES, MAX_TRANSACTION_BYTES, MAX_TRANSACTION_WRITES, PREFIX_BYTES, TRAILER_BYTES, FRAME, SEAL};
// A frozen test core keeps historical capacity-four experiments reproducible.
mod cap4 { const CAP:usize=4; include!("core.rs"); include!("checks.rs"); }
mod cap16 { const CAP:usize=16; include!("core.rs"); include!("checks.rs"); }
mod cap64 { const CAP:usize=64; include!("core.rs"); include!("checks.rs"); }
