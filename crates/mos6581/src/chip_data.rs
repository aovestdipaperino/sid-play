//! Chip constants whose values are borrowed (see docs/provenance.md). Read
//! through this module so the source can be switched in one place.

pub(crate) use sid_gpl_data::{
    ENV_EXPONENTIAL_BREAKPOINTS, ENV_RATE_COUNTER_SKIPS_ZERO, ENV_RATE_PERIOD, EXTFILT_W0_HIGH,
    EXTFILT_W0_LOW, FILTER_Q_MIN, FILTER_Q_STEPS, MIXER_DC, NOISE_CLOCK_BIT, NOISE_OUTPUT_TAPS,
    NOISE_SHIFT_RELOAD, VOICE_DC, VOICE_WAVE_ZERO,
};
