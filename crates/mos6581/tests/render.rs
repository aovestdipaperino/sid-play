use mos6581::{Model, Sid};

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

/// Renders one second with voice 1 as a gated sawtooth and voice 3 as a much faster
/// oscillator whose gate is *off*. A gated-off voice contributes only its DC term, so
/// voice 3 is inaudible while its oscillator still runs and still raises the MSB that
/// hard sync keys on. `sync` turns voice 1's sync bit on and off; everything else is
/// identical, so the two renders differ only by what sync does.
fn render_sync_case(sync: bool) -> Vec<f32> {
    let mut sid = Sid::new(Model::Pal, RATE);
    // Voice 3 (index 2): the master. Low frequency, gate off -- running but silent.
    sid.write(14, 0xa0);
    sid.write(15, 15); // voice 3 freq 4000, ~235 Hz at the PAL clock
    sid.write(18, 0x00); // no gate
    sid.write(19, 0x00);
    sid.write(20, 0x00);
    // Voice 2 (index 1): idle, contributes DC only.
    sid.write(7, 0);
    sid.write(8, 0);
    sid.write(11, 0);
    // Voice 1 (index 0): the slave. Low frequency, gated sawtooth.
    sid.write(0, 0xe8);
    sid.write(1, 3); // voice 1 freq 1000, ~59 Hz at the PAL clock
    sid.write(4, 0x21 | if sync { 0x02 } else { 0x00 }); // gate + sawtooth [+ sync]
    sid.write(5, 0x00);
    sid.write(6, 0xf0);
    sid.write(21, 0x07);
    sid.write(22, 0xff); // cutoff at the top
    sid.write(23, 0x01); // resonance 0, voice 1 routed
    sid.write(24, 0x1f); // low-pass tap, full volume

    let mut buf = vec![0.0f32; 44_100];
    sid.render(&mut buf);
    buf
}

/// How many times the signal crosses its own mean in a second. The slave free-runs at
/// ~59 Hz. Hard sync resets it every time the master's bit 23 rises, i.e. every
/// 2^24/4000 = 4194 cycles or ~235 Hz, and because the slave only reaches a quarter of
/// its range before being reset, the synced waveform repeats at the master's rate -- four
/// times as many mean crossings. The frequencies are deliberately not harmonically
/// locked: at an integer ratio the slave wraps of its own accord on exactly the cycle the
/// master resets it, and sync becomes inaudible. Measured through the chip's register
/// interface, not by driving the oscillator primitive.
fn zero_crossings(buf: &[f32]) -> usize {
    let tail = &buf[20_000..];
    let mean = tail.iter().sum::<f32>() / tail.len() as f32;
    let signed: Vec<f32> = tail.iter().map(|s| s - mean).collect();
    signed
        .windows(2)
        .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
        .count()
}

#[test]
fn hard_sync_locks_voice_1_to_voice_3() {
    let free = zero_crossings(&render_sync_case(false));
    let synced = zero_crossings(&render_sync_case(true));
    assert!(
        free > 30,
        "the free-running slave barely oscillated: {free} crossings"
    );
    assert!(
        synced > 3 * free,
        "sync did not raise the slave to its master's rate: free {free}, synced {synced}"
    );
}

/// Voice 1 as a gated triangle with voice 3 running slow and silent underneath, so the
/// only difference between the two renders is voice 1's ring-mod bit.
fn render_ring_case(ring: bool) -> Vec<f32> {
    let mut sid = Sid::new(Model::Pal, RATE);
    sid.write(14, 0xa0);
    sid.write(15, 15); // voice 3 freq 4000, gate off
    sid.write(18, 0x00);
    sid.write(19, 0x00);
    sid.write(20, 0x00);
    sid.write(7, 0);
    sid.write(8, 0);
    sid.write(11, 0);
    sid.write(0, 0xe8);
    sid.write(1, 3); // voice 1 freq 1000
    sid.write(4, 0x11 | if ring { 0x04 } else { 0x00 }); // gate + triangle [+ ring]
    sid.write(5, 0x00);
    sid.write(6, 0xf0);
    sid.write(21, 0x07);
    sid.write(22, 0xff);
    sid.write(23, 0x01);
    sid.write(24, 0x1f);

    let mut buf = vec![0.0f32; 44_100];
    sid.render(&mut buf);
    buf
}

#[test]
fn ring_modulation_of_voice_1_uses_voice_3_as_the_source() {
    let plain = render_ring_case(false);
    let ringed = render_ring_case(true);
    assert!(
        rms(&plain[20_000..]) > 0.01,
        "the control render was silent: {}",
        rms(&plain[20_000..])
    );
    assert!(
        rms(&ringed[20_000..]) > 0.01,
        "the ring-modulated render was silent: {}",
        rms(&ringed[20_000..])
    );
    let plain_x = zero_crossings(&plain);
    let ring_x = zero_crossings(&ringed);
    assert!(
        plain_x != ring_x,
        "ring modulation changed nothing: both cross {plain_x} times"
    );
}
