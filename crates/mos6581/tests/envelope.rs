use mos6581::envelope::{Adsr, Envelope};

const FAST: Adsr = Adsr {
    attack: 0x0,
    decay: 0x0,
    sustain: 0xf,
    release: 0x0,
};

/// Charge the envelope to full at the fastest rate.
fn charge(env: &mut Envelope) {
    let mut guard = 0;
    while env.level() != 255 && guard < 1_000_000 {
        env.tick(true, FAST);
        guard += 1;
    }
    assert_eq!(env.level(), 255, "envelope never reached full level");
}

/// Cycles for a gated attack from silence to 255 at the given rate nibble.
fn attack_cycles(nibble: u8) -> u64 {
    let adsr = Adsr {
        attack: nibble,
        ..Adsr::default()
    };
    let mut env = Envelope::default();
    let mut cycles = 0;
    while env.level() != 255 && cycles < 40_000_000 {
        env.tick(true, adsr);
        cycles += 1;
    }
    assert_eq!(env.level(), 255, "attack never completed");
    cycles
}

#[test]
fn attack_is_a_whole_number_of_rate_periods_and_slower_at_higher_nibbles() {
    // Attack steps once per rate period, so a full sweep is exactly 255 of them.
    let mut prev = 0;
    for nibble in 0..=0xfu8 {
        let cycles = attack_cycles(nibble);
        assert_eq!(cycles % 255, 0, "nibble {nibble}: {cycles} cycles");
        assert!(
            cycles > prev,
            "nibble {nibble} was not slower than the last"
        );
        prev = cycles;
    }
}

// Pins the measured rate periods, so it needs the borrowed data.
#[cfg(feature = "gpl-data")]
#[test]
fn attack_takes_255_steps_of_the_measured_rate_period() {
    use sid_gpl_data::ENV_RATE_PERIOD;
    for nibble in [0u8, 1, 0xf] {
        let period = u64::from(ENV_RATE_PERIOD[usize::from(nibble)]);
        assert_eq!(attack_cycles(nibble), 255 * period, "nibble {nibble}");
    }
}

/// Cycles for a release from 255 to 0 at the given release nibble.
fn release_cycles(nibble: u8) -> u64 {
    let release = Adsr {
        release: nibble,
        ..Adsr::default()
    };
    let mut env = Envelope::default();
    charge(&mut env);
    let mut cycles = 0;
    while env.level() != 0 && cycles < 40_000_000 {
        env.tick(false, release);
        cycles += 1;
    }
    assert_eq!(env.level(), 0, "release never reached zero");
    cycles
}

#[test]
fn a_falling_sweep_is_slower_than_the_same_rate_attack() {
    // Decay and release are exponential, so they take longer than the linear
    // attack at the same rate nibble.
    assert!(release_cycles(1) > attack_cycles(1));
}

// Pins the measured divider breakpoints, so it needs the borrowed data.
#[cfg(feature = "gpl-data")]
#[test]
fn a_falling_sweep_takes_the_rate_periods_the_breakpoints_imply() {
    use sid_gpl_data::{ENV_EXPONENTIAL_BREAKPOINTS, ENV_RATE_PERIOD};
    // The divider loads at each breakpoint level and holds until the next.
    let mut divider = 1u64;
    let mut periods = 0u64;
    for level in (1..=255u8).rev() {
        if let Some(e) = ENV_EXPONENTIAL_BREAKPOINTS.iter().find(|e| e.0 == level) {
            divider = u64::from(e.1);
        }
        periods += divider;
    }
    let nibble = 1u8;
    let period = u64::from(ENV_RATE_PERIOD[usize::from(nibble)]);
    assert_eq!(release_cycles(nibble), periods * period);
}

#[test]
fn sustain_target_is_the_nibble_times_seventeen() {
    for nibble in 0..=0xfu8 {
        let mut env = Envelope::default();
        charge(&mut env);
        let adsr = Adsr {
            decay: 0x0,
            sustain: nibble,
            ..Adsr::default()
        };
        let mut guard = 0;
        while env.level() != nibble * 17 && guard < 1_000_000 {
            env.tick(true, adsr);
            guard += 1;
        }
        assert_eq!(env.level(), nibble * 17, "sustain nibble {nibble}");

        // And it holds there rather than continuing down.
        let held = env.level();
        for _ in 0..50_000 {
            env.tick(true, adsr);
        }
        assert_eq!(env.level(), held, "sustain drifted, nibble {nibble}");
    }
}

#[test]
fn release_from_full_is_slower_than_release_from_low() {
    let release = Adsr {
        release: 0x8,
        ..Adsr::default()
    };

    let mut from_full = Envelope::default();
    charge(&mut from_full);
    let mut slow = 0;
    while from_full.level() != 0 && slow < 40_000_000 {
        from_full.tick(false, release);
        slow += 1;
    }

    let mut from_low = Envelope::default();
    let mut guard = 0;
    while from_low.level() < 40 && guard < 1_000_000 {
        from_low.tick(true, FAST);
        guard += 1;
    }
    let start = from_low.level();
    let mut fast = 0;
    while from_low.level() != 0 && fast < 40_000_000 {
        from_low.tick(false, release);
        fast += 1;
    }
    assert!(start > 0, "the low envelope never charged");
    assert!(
        slow > fast,
        "release from 255 took {slow}, from {start} took {fast}"
    );
}

#[test]
fn retriggering_before_the_release_reaches_zero_resumes_rather_than_resets() {
    // The counter is not cleared by gate-on. Only a release that actually reached
    // zero latches the generator, so a quick gate blip near full level keeps it there.
    let adsr = Adsr {
        attack: 0x0,
        decay: 0x0,
        sustain: 0xf,
        release: 0xf,
    };
    let mut env = Envelope::default();
    charge(&mut env);
    env.tick(false, adsr); // gate off
    env.tick(true, adsr); // gate on again, one cycle later
    assert_eq!(env.level(), 255, "gate-on must not jump the level");
}

#[test]
fn a_retrigger_after_a_completed_release_climbs_from_zero() {
    let adsr = Adsr {
        attack: 0x1,
        decay: 0x0,
        sustain: 0xf,
        release: 0x0,
    };
    let mut env = Envelope::default();
    charge(&mut env);
    let mut guard = 0;
    while env.level() != 0 && guard < 1_000_000 {
        env.tick(false, adsr);
        guard += 1;
    }
    assert_eq!(env.level(), 0, "release never reached zero");

    env.tick(true, adsr);
    assert_eq!(env.level(), 0, "gate-on must not jump the level");
    let mut cycles = 0;
    while env.level() != 255 && cycles < 1_000_000 {
        env.tick(true, adsr);
        cycles += 1;
    }
    assert_eq!(env.level(), 255, "retrigger never climbed");
    assert!(
        cycles > 255 * 16,
        "climb started above zero: {cycles} cycles"
    );
}

#[test]
fn an_envelope_that_was_never_gated_stays_at_zero() {
    let mut env = Envelope::default();
    for _ in 0..100_000 {
        env.tick(false, FAST);
    }
    assert_eq!(env.level(), 0);
}

#[test]
fn reset_returns_to_the_ungated_state() {
    let mut env = Envelope::default();
    charge(&mut env);
    env.reset();
    assert_eq!(env.level(), 0);
    for _ in 0..10_000 {
        env.tick(false, FAST);
    }
    assert_eq!(env.level(), 0, "reset envelope moved without a gate");
}

#[test]
fn register_nibbles_split_high_and_low() {
    let adsr = Adsr::from_regs(0x5a, 0x3c);
    assert_eq!(adsr.attack, 5);
    assert_eq!(adsr.decay, 0xa);
    assert_eq!(adsr.sustain, 3);
    assert_eq!(adsr.release, 0xc);
}

#[test]
fn rewriting_the_rate_register_to_a_faster_rate_delays_the_step() {
    // The ADSR-delay bug: the step fires only when the rate counter exactly
    // equals the period, so a counter already past a new, faster period has to
    // run on to the wrap before the step comes.
    let slow = Adsr {
        attack: 0xf,
        ..Adsr::default()
    };
    let fast = Adsr {
        attack: 0x0,
        ..Adsr::default()
    };
    let mut env = Envelope::default();
    for _ in 0..100 {
        env.tick(true, slow);
    }
    assert_eq!(env.level(), 0, "the slow rate stepped too early");
    let mut cycles = 1;
    env.tick(true, fast);
    while env.level() == 0 && cycles < 100_000 {
        env.tick(true, fast);
        cycles += 1;
    }
    assert_eq!(env.level(), 1);
    assert!(cycles > 10_000, "step came after only {cycles} cycles");
}

// The exact cycle count depends on the borrowed period and wrap constants.
#[cfg(feature = "gpl-data")]
#[test]
fn the_delayed_step_lands_on_the_counter_wrap_plus_the_new_period() {
    use sid_gpl_data::{ENV_RATE_COUNTER_SKIPS_ZERO, ENV_RATE_PERIOD};
    let slow = Adsr {
        attack: 0xf,
        ..Adsr::default()
    };
    let fast = Adsr {
        attack: 0x0,
        ..Adsr::default()
    };
    let fast_period = u32::from(ENV_RATE_PERIOD[0]);
    let warm = 100;

    let mut env = Envelope::default();
    for _ in 0..warm {
        env.tick(true, slow);
    }
    assert_eq!(env.level(), 0, "the slow rate stepped too early");

    let mut cycles = 1;
    env.tick(true, fast);
    assert_eq!(env.level(), 0, "the step ignored the delay bug");

    // The 15-bit counter wraps after (2^15 - warm) cycles, restarts at 0 or 1
    // depending on the wrap rule, then counts up to the new period.
    let restart = u32::from(ENV_RATE_COUNTER_SKIPS_ZERO);
    let expected = ((1u32 << 15) - warm) + (fast_period - restart);
    while env.level() == 0 && cycles < 100_000 {
        env.tick(true, fast);
        cycles += 1;
    }
    assert_eq!(cycles, expected);
    assert_eq!(env.level(), 1);
}
