//! Chip constants. With `gpl-data` they come from the borrowed data crate
//! (see docs/provenance.md); without it, from the clean-room defaults in
//! `clean_chip_data`. Read through this module so the source switches in one place.
//!
//! Public but hidden: it exists so integration tests can check that the code
//! uses whichever constants are active, in both builds. It is not a stable API.

#[cfg(not(feature = "gpl-data"))]
pub use crate::clean_chip_data::{
    ENV_EXPONENTIAL_BREAKPOINTS, ENV_RATE_COUNTER_SKIPS_ZERO, ENV_RATE_PERIOD, EXTFILT_W0_HIGH,
    EXTFILT_W0_LOW, FILTER_Q_MIN, FILTER_Q_STEPS, MIXER_DC, NOISE_CLOCK_BIT, NOISE_OUTPUT_TAPS,
    NOISE_SHIFT_RELOAD, VOICE_DC, VOICE_WAVE_ZERO,
};
#[cfg(feature = "gpl-data")]
pub use sid_gpl_data::{
    ENV_EXPONENTIAL_BREAKPOINTS, ENV_RATE_COUNTER_SKIPS_ZERO, ENV_RATE_PERIOD, EXTFILT_W0_HIGH,
    EXTFILT_W0_LOW, FILTER_Q_MIN, FILTER_Q_STEPS, MIXER_DC, NOISE_CLOCK_BIT, NOISE_OUTPUT_TAPS,
    NOISE_SHIFT_RELOAD, VOICE_DC, VOICE_WAVE_ZERO,
};
