//! Gate-driven envelope generator, clocked once per Φ2 cycle.

/// The four register nibbles: `$d402` attack/decay, `$d403` sustain/release.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Adsr {
    pub attack: u8,
    pub decay: u8,
    pub sustain: u8,
    pub release: u8,
}

impl Adsr {
    pub fn from_regs(attack_decay: u8, sustain_release: u8) -> Self {
        Self {
            attack: attack_decay >> 4,
            decay: attack_decay & 0x0f,
            sustain: sustain_release >> 4,
            release: sustain_release & 0x0f,
        }
    }
}

/// Rate-counter periods per ADSR nibble, measured on real 6581 hardware.
const RATE_PERIOD: [u16; 16] = [
    9, 32, 63, 95, 149, 220, 267, 313, 392, 977, 1954, 3126, 3907, 11720, 19532, 31251,
];

/// Exponential-counter period loaded at each envelope counter value. The shape of
/// the decay and release comes from this divider rather than from a curve; the
/// period for a given value can differ between a rising and a falling envelope,
/// which is why the table is only consulted on the falling phases.
const EXPONENTIAL_PERIOD: [u16; 256] = {
    let mut t = [0u16; 256];
    t[0] = 1;
    t[6] = 30;
    t[14] = 16;
    t[26] = 8;
    t[54] = 4;
    t[93] = 2;
    t[255] = 1;
    t
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Attack,
    DecaySustain,
    Release,
}

#[derive(Debug, Clone, Copy)]
pub struct Envelope {
    counter: u8,
    rate_counter: u16,
    exponential_counter: u16,
    exponential_period: u16,
    state: State,
    hold_zero: bool,
    gate_prev: bool,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            counter: 0,
            rate_counter: 0,
            exponential_counter: 0,
            exponential_period: 1,
            state: State::Release,
            hold_zero: true,
            gate_prev: false,
        }
    }
}

impl Envelope {
    /// The 8-bit envelope level, 0..=255.
    #[inline]
    pub fn level(&self) -> u8 {
        self.counter
    }

    /// One Φ2 cycle.
    pub fn tick(&mut self, gate: bool, adsr: Adsr) {
        if gate && !self.gate_prev {
            self.state = State::Attack;
            self.exponential_counter = 0;
            self.exponential_period = 1;
            self.hold_zero = false;
        } else if !gate && self.gate_prev && self.state != State::Release {
            self.state = State::Release;
            self.exponential_counter = 0;
        }
        self.gate_prev = gate;

        if self.hold_zero {
            return;
        }

        let period = match self.state {
            State::Attack => RATE_PERIOD[usize::from(adsr.attack & 0x0f)],
            State::DecaySustain => RATE_PERIOD[usize::from(adsr.decay & 0x0f)],
            State::Release => RATE_PERIOD[usize::from(adsr.release & 0x0f)],
        };

        // The rate counter is 15 bits wide and the step fires on exact equality,
        // which is the ADSR-delay bug: rewrite the register so the new period is
        // below the counter's current value and it keeps counting to the wrap,
        // resets, and only then counts up to the new period before stepping.
        self.rate_counter += 1;
        if self.rate_counter & 0x8000 != 0 {
            self.rate_counter = (self.rate_counter + 1) & 0x7fff;
        }
        if self.rate_counter != period {
            return;
        }
        self.rate_counter = 0;

        // Attack steps once per rate period; the exponential divider is only in
        // the decay and release paths.
        if self.state == State::Attack {
            self.attack_step();
            return;
        }

        self.exponential_counter += 1;
        if self.exponential_counter < self.exponential_period {
            return;
        }
        self.exponential_counter = 0;

        match self.state {
            State::DecaySustain => {
                let target = (adsr.sustain & 0x0f) * 17;
                if self.counter != target && self.counter != 0 {
                    self.counter -= 1;
                    self.load_period();
                }
                if self.counter == 0 {
                    self.hold_zero = true;
                }
            }
            State::Release => {
                if self.counter != 0 {
                    self.counter -= 1;
                    self.load_period();
                }
                if self.counter == 0 {
                    self.hold_zero = true;
                }
            }
            State::Attack => {}
        }
    }

    fn attack_step(&mut self) {
        if self.counter == 255 {
            self.state = State::DecaySustain;
            self.exponential_counter = 0;
            self.exponential_period = 1;
        } else {
            self.counter += 1;
        }
    }

    #[inline]
    fn load_period(&mut self) {
        let p = EXPONENTIAL_PERIOD[usize::from(self.counter)];
        if p != 0 {
            self.exponential_period = p;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
