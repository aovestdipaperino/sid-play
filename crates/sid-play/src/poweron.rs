//! The RAM pattern at `$0000-$03FF` before a tune's init runs.

/// With `gpl-data`: libsidplayfp's captured pattern (GPL-2.0, via
/// `sid-gpl-data`). Without it: zeros; the harness still writes the Kernal
/// pointers it sets for every tune.
#[cfg(feature = "gpl-data")]
pub const POWERON: [u8; 0x0400] = sid_gpl_data::POWERON;
#[cfg(not(feature = "gpl-data"))]
pub const POWERON: [u8; 0x0400] = [0; 0x0400];
