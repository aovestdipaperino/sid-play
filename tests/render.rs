use rust_sid::{Model, Sid};

const RATE: f64 = 44_100.0;

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
}

/// Voice 1, gated triangle+saw, through the filter at full cutoff.
fn play_tone(sid: &mut Sid, freq: u16) {
    sid.write(0, (freq & 0xff) as u8);
    sid.write(1, (freq >> 8) as u8);
    sid.write(4, 0x21); // gate + sawtooth
    sid.write(5, 0x00); // fast attack, fast decay
    sid.write(6, 0xf0); // full sustain
    sid.write(21, 0x07);
    sid.write(22, 0xff); // cutoff at the top
    sid.write(23, 0x01); // resonance 0, voice 1 routed
    sid.write(24, 0x1f); // low-pass tap, full volume
}

#[test]
fn registers_read_back() {
    let mut sid = Sid::new(Model::Pal, RATE);
    for addr in 0..25 {
        sid.write(addr, 0x5a);
        assert_eq!(sid.read(addr), 0x5a, "register {addr}");
    }
}

#[test]
fn a_fresh_chip_is_silent() {
    let mut sid = Sid::new(Model::Pal, RATE);
    let mut buf = vec![1.0f32; 4_000];
    sid.render(&mut buf);
    assert!(buf.iter().all(|&s| s == 0.0));
}

#[test]
fn a_gated_tone_makes_sound() {
    let mut sid = Sid::new(Model::Pal, RATE);
    play_tone(&mut sid, 3_740);
    let mut buf = vec![0.0f32; 44_100];
    sid.render(&mut buf);
    assert!(
        rms(&buf[1000..]) > 0.01,
        "tone produced rms {}",
        rms(&buf[1000..])
    );
}

/// The measured 6581 curve bottoms out at 220 Hz, so a 220 Hz fundamental sits *at*
/// the closed corner and is not the right probe. A tone 17 times above it is: the
/// second-order stage has to take it down by roughly the square of that ratio, which
/// is what this asserts.
#[test]
fn the_filter_actually_filters() {
    let probe = 65_000u16; // ~3.8 kHz at the PAL clock, 17x above the closed corner

    let mut open = Sid::new(Model::Pal, RATE);
    play_tone(&mut open, probe);
    let mut a = vec![0.0f32; 44_100];
    open.render(&mut a);

    let mut shut = Sid::new(Model::Pal, RATE);
    play_tone(&mut shut, probe);
    shut.write(21, 0x00);
    shut.write(22, 0x00);
    let mut b = vec![0.0f32; 44_100];
    shut.render(&mut b);

    let (open_rms, shut_rms) = (rms(&a[1000..]), rms(&b[1000..]));
    assert!(
        open_rms > 0.01,
        "nothing came through at full cutoff: {open_rms}"
    );
    assert!(
        shut_rms < open_rms / 4.0,
        "cutoff had no audible effect: shut {shut_rms} open {open_rms}"
    );
}

#[test]
fn render_cycles_emits_the_right_amount_of_audio() {
    let mut sid = Sid::new(Model::Pal, RATE);
    let mut out = Vec::new();
    sid.render_cycles(985_248, &mut out);
    assert!(
        out.len() > 43_000 && out.len() < 45_200,
        "got {} samples",
        out.len()
    );
}

#[test]
fn voice3_reads_back_its_noise_and_envelope() {
    let mut sid = Sid::new(Model::Pal, RATE);
    sid.write(4 + 14, 0x81); // voice 3: gate + noise
    sid.write(5 + 14, 0x00);
    sid.write(6 + 14, 0xf0);
    let mut buf = vec![0.0f32; 4_000];
    sid.render(&mut buf);
    assert_ne!(sid.read(27), 0, "$d41b should expose the noise register");
    assert!(sid.read(28) > 0, "$d41c should expose the envelope level");
}

#[test]
fn reset_clears_everything() {
    let mut sid = Sid::new(Model::Pal, RATE);
    play_tone(&mut sid, 3_740);
    let mut buf = vec![0.0f32; 4_000];
    sid.render(&mut buf);
    sid.reset();
    assert_eq!(sid.read(4), 0);
    sid.render(&mut buf);
    assert!(buf.iter().all(|&s| s == 0.0), "sound survived reset");
}

/// The filter keeps volume, routing and taps across its own reset, so the chip has
/// to re-apply them from the register file. A stale volume would leave this audible.
#[test]
fn reset_silences_a_chip_that_was_playing_at_full_volume() {
    let mut sid = Sid::new(Model::Pal, RATE);
    play_tone(&mut sid, 3_740);
    let mut buf = vec![0.0f32; 44_100];
    sid.render(&mut buf);
    assert!(
        rms(&buf[1000..]) > 0.01,
        "chip was never audible to begin with"
    );
    sid.reset();
    sid.render(&mut buf);
    assert!(
        buf.iter().all(|&s| s == 0.0),
        "volume or routing survived the reset: rms {}",
        rms(&buf)
    );
}

/// `$d417` carries the routing mask as well as the resonance, so writing it has to
/// move the routing too. With no taps enabled a routed voice has nowhere to go.
#[test]
fn routing_through_the_filter_takes_effect_on_the_write() {
    let mut sid = Sid::new(Model::Pal, RATE);
    sid.write(0, (3_740 & 0xff) as u8);
    sid.write(1, (3_740 >> 8) as u8);
    sid.write(4, 0x21); // gate + sawtooth
    sid.write(5, 0x00);
    sid.write(6, 0xf0);
    sid.write(21, 0x07);
    sid.write(22, 0xff);
    sid.write(24, 0x0f); // full volume, no taps: everything goes direct

    let mut buf = vec![0.0f32; 44_100];
    sid.render(&mut buf);
    let direct = rms(&buf[1000..]);
    assert!(direct > 0.01, "unrouted voice was silent: {direct}");

    sid.write(23, 0x01); // route voice 1 through the filter, resonance 0
    let mut after = vec![0.0f32; 44_100];
    sid.render(&mut after);
    // Measured well past the output stage's 9.7 ms high-pass, so the step from the
    // routing change itself has long since decayed.
    let routed = rms(&after[20_000..]);
    assert!(
        routed < 1e-6,
        "routing written to $d417 was ignored: rms {routed} after {direct}"
    );
}

/// Three voices at full envelope each swing about one unit of `FULL_SCALE`, so the
/// raw mix peaks near three. The output scaling has to bring that inside the range
/// the sink assumes, and it has to do it for the worst case, not the average one.
#[test]
fn a_full_three_voice_mix_does_not_clip() {
    let mut sid = Sid::new(Model::Pal, RATE);
    for v in 0..3 {
        let base = v * 7;
        // Identical frequency and start phase for all three: they sum in phase,
        // which is the loudest the mixer can get.
        sid.write(base, (3_740 & 0xff) as u8);
        sid.write(base + 1, (3_740 >> 8) as u8);
        sid.write(base + 4, 0x21); // gate + sawtooth
        sid.write(base + 5, 0x00);
        sid.write(base + 6, 0xf0);
    }
    sid.write(21, 0x07);
    sid.write(22, 0xff);
    sid.write(23, 0x07); // all three routed, resonance 0
    sid.write(24, 0x1f); // low-pass tap, full volume

    let mut buf = vec![0.0f32; 44_100];
    sid.render(&mut buf);
    let tail = &buf[20_000..];
    let peak = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let level = rms(tail);
    assert!(level > 0.01, "three-voice mix was silent: rms {level}");
    assert!(peak < 1.0, "three-voice mix clips: peak {peak}");
}

/// The chip is constructed by callers that have no idea what audio hardware exists,
/// so a nonsense host rate must degrade rather than abort.
#[test]
fn an_absurd_host_rate_still_produces_a_working_chip() {
    for rate in [0.0, -1.0, f64::NAN, 1.0e-30, 1.0e9, f64::INFINITY] {
        let mut sid = Sid::new(Model::Pal, rate);
        play_tone(&mut sid, 3_740);
        let mut buf = vec![0.0f32; 2_000];
        sid.render(&mut buf);
        assert!(
            buf.iter().all(|s| s.is_finite()),
            "rate {rate} produced non-finite output"
        );
    }
}

#[test]
fn a_rate_above_the_chip_clock_is_reported_by_try_new() {
    assert!(Sid::try_new(Model::Pal, 2_000_000.0).is_err());
    assert!(Sid::try_new(Model::Pal, 44_100.0).is_ok());
}
