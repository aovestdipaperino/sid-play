//! The RAM pattern at `$0000-$03FF` before a tune's init runs.

/// The clean-room documented power-on image. The harness still writes its
/// per-tune Kernal pointers on top of it.
pub const CLEAN_POWERON: [u8; 0x0400] = crate::poweron_clean::POWERON;

/// The image every render starts from: [`CLEAN_POWERON`]. `render --poweron
/// FILE` replaces it with a 1024-byte image read from a file.
pub const POWERON: [u8; 0x0400] = CLEAN_POWERON;
