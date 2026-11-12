//! Play C64 PSID tunes through the `mos6581` SID emulator.
//!
//! The default build is GPL-free (MIT OR Apache-2.0). The off-by-default
//! `gpl-data` feature swaps in measured reSID data and the libsidplayfp power-on
//! pattern and makes the build GPL-2.0. Libraries should never enable
//! `gpl-data`; leave the choice to the final binary. Cargo unifies features, so
//! one dependency enabling it makes the whole program GPL-2.0.

#[doc(hidden)]
pub mod poweron;
#[doc(hidden)]
pub mod poweron_clean;
pub mod sink;
