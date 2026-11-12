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
fn attack_ignores_the_exponential_divider() {
    // Attack steps once per rate period, so a full sweep is exactly 255 of them.
    // At nibble 1 the rate period is 32 cycles.
    assert_eq!(attack_cycles(1), 255 * 32);
}

#[test]
fn attack_at_the_fastest_rate_is_the_same_255_steps() {
    assert_eq!(attack_cycles(0), 255 * 9);
}

#[test]
fn a_falling_sweep_takes_the_measured_756_rate_periods() {
    // Measured on real silicon: a complete falling envelope walks
    // 162*1 + 39*2 + 28*4 + 12*8 + 8*16 + 6*30 = 756 rate-counter periods,
    // because the exponential divider loads 1,2,4,8,16,30 at counter values
    // 255,93,54,26,14,6. At nibble 1 (period 32) that is 32_352 cycles.
    let release = Adsr {
        release: 0x1,
        ..Adsr::default()
    };
    let mut env = Envelope::default();
    charge(&mut env);

    let mut cycles = 0;
    while env.level() != 0 && cycles < 40_000_000 {
        env.tick(false, release);
        cycles += 1;
    }
    assert_eq!(cycles, 756 * 32, "falling sweep is not 756 periods");
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
fn rewriting_the_rate_register_mid_envelope_delays_the_step_to_the_counter_wrap() {
    // The ADSR-delay bug. The rate counter is 15 bits and the envelope step fires
    // only when it exactly equals the current period, so switching to a faster
    // rate whose period the counter has already passed does not step on the next
    // cycle: the counter runs on to 0x8000, wraps to 1, and only then counts up to
    // the new period.
    let slow = Adsr {
        attack: 0xf, // period 31_251
        ..Adsr::default()
    };
    let fast = Adsr {
        attack: 0x0, // period 9
        ..Adsr::default()
    };
    let warm = 100;

    let mut env = Envelope::default();
    for _ in 0..warm {
        env.tick(true, slow);
    }
    assert_eq!(env.level(), 0, "the slow rate stepped too early");

    // One cycle at the new, faster rate must still not step.
    let mut cycles = 1;
    env.tick(true, fast);
    assert_eq!(env.level(), 0, "the step ignored the delay bug");

    // From a counter of `warm`: (0x8000 - warm) cycles to reach the wrap, which
    // lands on 1 rather than 0, then (period - 1) more to reach the period.
    let expected = (0x8000 - warm) + (9 - 1);
    while env.level() == 0 && cycles < 40_000 {
        env.tick(true, fast);
        cycles += 1;
    }
    assert_eq!(
        cycles, expected,
        "step came after {cycles} cycles, expected {expected} from the wrap"
    );
    assert_eq!(env.level(), 1);
}
