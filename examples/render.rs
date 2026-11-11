//! PSID harness: runs a tune's 6510 code and feeds the writes to the chip.
//!
//! The processor runs one Φ2 cycle at a time and the chip is rendered alongside
//! it, so a register write lands in the same cycle the tune wrote it. The tune's
//! own code owns everything it was loaded over, so the harness keeps its
//! footprint to the top of the last page: call stubs, an interrupt handler for
//! tunes that do not install one, and a halt loop, all above $ffe0.
//!
//! `cargo run --release --example render -- tune.sid [--subsong N] [--seconds S]
//!         [--ntsc] [--out file.wav]`

use lib6502::Cpu;
use lib6502::Variant;
use lib6502::bus::Bus;
use rust_sid::sink::{SampleFormat, Sink, WavSink, WavSpec};
use rust_sid::{Model, Sid};
use std::cell::{Cell, RefCell};
use std::error::Error;
use std::path::PathBuf;
use std::rc::Rc;

const RATE: f64 = 44_100.0;

/// `JSR <entry>` then `JMP HALT`, so the tune's `RTS` brings us back to the stub
/// and the stub lands us on the halt address. Using the processor's own `JSR`
/// means the stack frame is laid out exactly as the tune expects it.
const STUB_INIT: u16 = 0xffe0;
/// `JSR play` then `JMP HALT`, entered by the harness once a frame has passed,
/// for a tune that brought no interrupt handler of its own. It is entered at an
/// instruction boundary with the processor idle, and the tune's `RTS` returns it
/// to the stub, which puts it back on the halt loop.
const STUB_PLAY: u16 = 0xffe8;
/// Where the processor idles between interrupts.
const HALT: u16 = 0xfff8;

const IRQ_VECTOR: u16 = 0xfffe;
const RESET_VECTOR: u16 = 0xfffc;

/// Status bits, as the processor lays them out.
const I_FLAG: u8 = 0x04;
const B_FLAG: u8 = 0x10;
const U_FLAG: u8 = 0x20;
/// The 6502 interrupt entry takes seven cycles.
const IRQ_CYCLES: u8 = 7;
/// The stack lives on page 1.
const STACK_PAGE: u16 = 0x0100;

/// Cycles a candidate load is given to prove itself: init, then a fraction of a
/// second of playing.
const TRIAL_CYCLES: u64 = 500_000;

/// CIA 1 timer A, modelled to the point where it can raise an interrupt.
///
/// Only what a tune can observe is implemented: the latch, the control
/// register's start bit, and the interrupt control register with its read-to-
/// clear behaviour. The control register's other bits — port B latching, the TOD
/// and serial sources, PB7 output — are stored but have no effect, because this
/// harness has no port B and no clock.
#[derive(Default)]
struct Cia {
    latch: u16,
    counter: u16,
    control: u8,
    /// Set flags; bit 0 is timer A underflow.
    flags: u8,
    /// Which flags are allowed to pull the interrupt line.
    mask: u8,
    /// True once a tune has started the timer, so a tune that never touches the
    /// CIA can be played at a fixed frame rate instead.
    started: bool,
}

impl Cia {
    fn timer_a_running(&self) -> bool {
        self.control & 0x01 != 0
    }

    /// The CIA's output: asserted while any enabled flag is set.
    fn line(&self) -> bool {
        self.flags & self.mask != 0
    }

    /// Advance timer A by one cycle. Returns true on the cycle the line rises.
    fn tick(&mut self) -> bool {
        let before = self.line();
        if self.timer_a_running() {
            if self.counter == 0 {
                self.flags |= 0x01;
                self.counter = self.latch;
            } else {
                self.counter -= 1;
            }
        }
        self.line() && !before
    }

    /// Reading the interrupt register reports the flags and clears them, which
    /// is what releases the interrupt line. A handler that never gets a cleared
    /// read either re-interrupts forever or never at all.
    fn read_icr(&mut self) -> u8 {
        let value = self.flags | 0x80;
        self.flags = 0;
        value
    }

    fn write_icr(&mut self, value: u8) {
        if value & 0x80 != 0 {
            self.mask |= value & 0x1f;
        } else {
            self.mask &= !(value & 0x1f);
        }
    }

    fn write_control(&mut self, value: u8) {
        let was_running = self.timer_a_running();
        self.control = value;
        if self.timer_a_running() {
            if !was_running {
                // Starting the timer loads the counter from the latch.
                self.counter = self.latch;
            }
            self.started = true;
        } else {
            self.counter = self.latch;
        }
    }
}

struct Machine {
    ram: [u8; 0x1_0000],
    sid: Sid,
    cia: RefCell<Cia>,
    /// How many writes have reached the chip. The loader uses it to tell a
    /// mapping that plays a tune from one that runs silence.
    sid_writes: Cell<u64>,
}

impl Machine {
    fn new(model: Model) -> Self {
        Self {
            ram: [0; 0x1_0000],
            sid: Sid::new(model, RATE),
            cia: RefCell::new(Cia::default()),
            sid_writes: Cell::new(0),
        }
    }
}

impl Bus for Machine {
    fn read(&self, address: u16) -> u8 {
        let a = usize::from(address);
        match a {
            0xd400..=0xd5ff => self.sid.read(a - 0xd400),
            0xdc00..=0xdc0f => match a - 0xdc00 {
                4 => (self.cia.borrow().counter & 0xff) as u8,
                5 => (self.cia.borrow().counter >> 8) as u8,
                0xd => self.cia.borrow_mut().read_icr(),
                0xe => self.cia.borrow().control,
                _ => 0,
            },
            _ => self.ram[a],
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        let a = usize::from(address);
        match a {
            0xd400..=0xd5ff => {
                self.sid_writes.set(self.sid_writes.get() + 1);
                self.sid.write(a - 0xd400, value);
            }
            0xdc00..=0xdc0f => match a - 0xdc00 {
                4 | 5 => {
                    let mut cia = self.cia.borrow_mut();
                    if a - 0xdc00 == 4 {
                        cia.latch = (cia.latch & 0xff00) | u16::from(value);
                    } else {
                        cia.latch = (cia.latch & 0x00ff) | (u16::from(value) << 8);
                    }
                    if !cia.timer_a_running() {
                        cia.counter = cia.latch;
                    }
                }
                0xd => self.cia.borrow_mut().write_icr(value),
                0xe => self.cia.borrow_mut().write_control(value),
                _ => {}
            },
            // $01 is the CPU port register. Its bank bits select RAM areas that
            // this flat address space already behaves as, so it is stored as
            // ordinary memory and nothing further happens.
            _ => self.ram[a] = value,
        }
    }

    /// The harness loads the tune itself, out of the PSID container.
    fn load(&mut self, _path: &str, _address: u16) {}
}

/// The bus handle the processor holds. The CPU owns its bus, so this is how the
/// harness keeps reaching the same chip and memory the tune is driving.
struct BusHandle(Rc<RefCell<Machine>>);

impl Bus for BusHandle {
    fn read(&self, address: u16) -> u8 {
        self.0.borrow().read(address)
    }

    fn write(&mut self, address: u16, value: u8) {
        self.0.borrow_mut().write(address, value);
    }

    fn load(&mut self, path: &str, address: u16) {
        self.0.borrow_mut().load(path, address);
    }
}

fn write_call_stub(ram: &mut [u8; 0x1_0000], stub: u16, entry: u16) {
    let s = usize::from(stub);
    ram[s] = 0x20; // JSR absolute
    ram[s + 1] = (entry & 0xff) as u8;
    ram[s + 2] = (entry >> 8) as u8;
    ram[s + 3] = 0x4c; // JMP absolute
    ram[s + 4] = (HALT & 0xff) as u8;
    ram[s + 5] = (HALT >> 8) as u8;
}

/// `JSR play` then `JMP HALT`, for a tune that leaves the interrupt vector alone.
fn write_play_stub(ram: &mut [u8; 0x1_0000], play: u16) {
    write_call_stub(ram, STUB_PLAY, play);
}

/// `JMP HALT`, so an idle processor stays put instead of executing whatever the
/// zeros in memory would have it run.
fn write_halt_loop(ram: &mut [u8; 0x1_0000]) {
    let s = usize::from(HALT);
    ram[s] = 0x4c;
    ram[s + 1] = (HALT & 0xff) as u8;
    ram[s + 2] = (HALT >> 8) as u8;
}

fn push_byte(cpu: &mut Cpu, machine: &mut Machine, value: u8) {
    machine.write(STACK_PAGE + u16::from(cpu.sp), value);
    cpu.sp = cpu.sp.wrapping_sub(1);
}

/// Enter a hardware IRQ the way the 6502 does.
///
/// The processor crate's own entry point is wrong for this: it raises the
/// disable flag *before* pushing the status byte, so the saved status carries
/// I set, every `RTI` restores interrupts masked, and only one interrupt can
/// ever be taken. Hardware pushes the status as it was at the moment of the
/// interrupt. The registers and the stack are both reachable from here, so the
/// frame is pushed by hand: return address high, then low, then status with B
/// clear and the unused bit set, exactly as `RTI` expects to find it.
fn irq_entry(cpu: &mut Cpu, machine: &mut Machine) {
    let return_pc = cpu.pc;
    // Only taken while I is clear, so the pushed status carries I clear too.
    let pushed_status = (cpu.status & !B_FLAG) | U_FLAG;
    let vector = u16::from_le_bytes([machine.read(IRQ_VECTOR), machine.read(IRQ_VECTOR + 1)]);

    push_byte(cpu, machine, (return_pc >> 8) as u8);
    push_byte(cpu, machine, (return_pc & 0xff) as u8);
    push_byte(cpu, machine, pushed_status);

    cpu.status |= I_FLAG;
    cpu.pc = vector;
    cpu.cycles = IRQ_CYCLES;
}

struct Options {
    tune: Option<PathBuf>,
    /// Left unset to mean "use whatever the tune's own header asks for".
    subsong: Option<u8>,
    seconds: f64,
    ntsc: bool,
    out: PathBuf,
}

fn parse_args() -> Result<Options, Box<dyn Error>> {
    let mut o = Options {
        tune: None,
        subsong: None,
        seconds: 30.0,
        ntsc: false,
        out: PathBuf::from("out.wav"),
    };

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--subsong" => {
                let v: u8 = args.next().ok_or("--subsong needs a value")?.parse()?;
                o.subsong = Some(v);
            }
            "--seconds" => o.seconds = args.next().ok_or("--seconds needs a value")?.parse()?,
            "--out" => o.out = PathBuf::from(args.next().ok_or("--out needs a value")?),
            "--ntsc" => o.ntsc = true,
            other => o.tune = Some(PathBuf::from(other)),
        }
    }

    Ok(o)
}

/// A PSID header's load-relevant fields.
struct Psid {
    data_off: usize,
    data_len: usize,
    init: u16,
    play: u16,
    load: u16,
    /// Which subsong the tune asks to start on. The harness passes this to the
    /// init routine the same way it passes an explicit `--subsong`, because a
    /// tune's own table of song offsets is indexed by that value.
    start_subsong: u8,
}

fn parse_header(header: &[u8]) -> Psid {
    let u16be = |o: usize| u16::from_be_bytes([header[o], header[o + 1]]);
    // The data offset is measured from the start of the C64-side header, which
    // begins after the four-byte file magic. A zero data length means the block
    // runs to the end of the file.
    let data_off = 4 + usize::from(u16be(0x06));
    Psid {
        data_off,
        data_len: match usize::from(u16be(0x08)) {
            0 => usize::MAX,
            n => n,
        },
        init: u16be(0x0a),
        play: u16be(0x0c),
        load: u16be(0x0e),
        start_subsong: header[0x76],
    }
}

/// One way of placing the tune's block in memory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Placement {
    load: u16,
    offset: usize,
}

/// The placements worth trying, declared one first.
///
/// A load address below the stack page cannot be right for a block of any size:
/// it would put the tune's code over the zero page and the stack it uses to
/// sequence itself. For those, the routine addresses in the header say where the
/// block really sits, so try the page the lower of init and play names. The data
/// offset gets the same treatment at a smaller scale, because a block written
/// out against a header of a different size is off by that size.
fn placements(psid: &Psid, bytes: &[u8]) -> Vec<Placement> {
    let mut loads = vec![psid.load];
    if psid.load < 0x0100 {
        let base = psid.init.min(psid.play) & 0xff00;
        if base != 0 && base != psid.load {
            loads.push(base);
        }
    }

    let mut out = Vec::new();
    for load in loads {
        if usize::from(load) >= 0xd000 {
            continue;
        }
        for offset in [
            psid.data_off,
            psid.data_off.wrapping_sub(2),
            psid.data_off.wrapping_add(2),
        ] {
            // A block starting before the file's own header ends is not a block.
            if offset < 4 || offset >= bytes.len() {
                continue;
            }
            if out
                .iter()
                .all(|p: &Placement| *p != Placement { load, offset })
            {
                out.push(Placement { load, offset });
            }
        }
    }
    out
}

/// Place one candidate mapping in memory and lay down the harness's own stubs.
fn setup(
    model: Model,
    bytes: &[u8],
    psid: &Psid,
    place: &Placement,
) -> Option<Rc<RefCell<Machine>>> {
    let load = usize::from(place.load);
    let len = psid
        .data_len
        .min(bytes.len() - place.offset)
        .min(0xd000 - load);
    if len == 0 {
        return None;
    }
    let machine = Rc::new(RefCell::new(Machine::new(model)));
    {
        let mut m = machine.borrow_mut();
        m.ram[load..load + len].copy_from_slice(&bytes[place.offset..place.offset + len]);
        write_call_stub(&mut m.ram, STUB_INIT, psid.init);
        write_play_stub(&mut m.ram, psid.play);
        write_halt_loop(&mut m.ram);
        // Point the interrupt vector at the halt loop, so a tune that never
        // installs a handler leaves it there and the harness can tell. A tune
        // that does install one overwrites this and owns its own timing.
        m.ram[usize::from(IRQ_VECTOR)] = (HALT & 0xff) as u8;
        m.ram[usize::from(IRQ_VECTOR + 1)] = (HALT >> 8) as u8;
        m.ram[usize::from(RESET_VECTOR)] = (HALT & 0xff) as u8;
        m.ram[usize::from(RESET_VECTOR + 1)] = (HALT >> 8) as u8;
    }
    Some(machine)
}

/// Start a processor on a prepared machine and run its init to completion.
fn start(machine: &Rc<RefCell<Machine>>, clock_hz: f64, subsong: u8) -> (Cpu, Harness) {
    let mut cpu = Cpu::new(Box::new(BusHandle(Rc::clone(machine))), 0);
    cpu.change_variant(Variant::NMOS);
    cpu.set_illegal_opcodes(true);
    cpu.reset();
    // The PSID convention passes the starting subsong to the init routine in A.
    cpu.a = subsong;
    cpu.pc = STUB_INIT;

    let mut harness = Harness::new(Rc::clone(machine), clock_hz);
    let mut budget = 20_000_000u64;
    while cpu.pc != HALT && budget > 0 {
        harness.cycle(&mut cpu);
        budget -= 1;
    }
    // Whose handler is installed decides who drives the frame.
    harness.frame_clock = harness.irq_vector() == HALT;
    (cpu, harness)
}

/// Load one placement and run the tune briefly, reporting how much it reached
/// the chip.
///
/// Whether a mapping is right is settled by what the tune does under it, not by
/// reading its bytes: a block placed where its own header did not mean it runs
/// into data and writes nothing, and that is observable.
fn trial(
    model: Model,
    bytes: &[u8],
    psid: &Psid,
    place: &Placement,
    subsong: u8,
    clock_hz: f64,
) -> Option<(u64, Rc<RefCell<Machine>>)> {
    let machine = setup(model, bytes, psid, place)?;
    let (mut cpu, mut harness) = start(&machine, clock_hz, subsong);
    for _ in 0..TRIAL_CYCLES {
        harness.cycle(&mut cpu);
    }
    let writes = machine.borrow().sid_writes.get();
    Some((writes, machine))
}

/// Everything the cycle loop carries between iterations.
struct Harness {
    machine: Rc<RefCell<Machine>>,
    /// Cycles since the last harness-generated frame tick, and a tick that has
    /// arrived but not yet be delivered.
    frame_cycles: u64,
    frame_due: bool,
    frame_period: u64,
    /// Only a tune that left the interrupt vector to us is played on a harness
    /// clock; a tune that installed its own handler owns its own timing, and
    /// ticking it as well would call a play routine it is not using.
    frame_clock: bool,
    /// Interrupts taken and frames driven, for the summary line.
    irqs: u64,
    frames: u64,
    audio: Vec<f32>,
}

impl Harness {
    fn new(machine: Rc<RefCell<Machine>>, clock_hz: f64) -> Self {
        Self {
            machine,
            frame_cycles: 0,
            frame_due: false,
            frame_period: (clock_hz / 60.0) as u64,
            frame_clock: false,
            irqs: 0,
            frames: 0,
            audio: Vec::with_capacity(4096),
        }
    }

    /// One Φ2 cycle: advance the CIA, offer any pending interrupt at an
    /// instruction boundary, step the processor, then let the chip run the cycle
    /// it just saw. An instruction in progress always finishes, which is why the
    /// request is only offered when the processor has no cycles left to spend.
    fn cycle(&mut self, cpu: &mut Cpu) {
        let underflow = self.machine.borrow().cia.borrow_mut().tick();
        if self.frame_clock {
            // The tune brought no handler, so the harness is the player. It
            // cannot wait on the processor's interrupt flag, because a tune
            // initialised with interrupts masked never clears it — a real
            // machine reaches the player through the Kernal's own chain, which
            // runs with interrupts enabled by then. Enter the play stub
            // directly, at an instruction boundary, with the processor idle.
            if self.machine.borrow().cia.borrow().started {
                self.frame_due |= underflow;
            } else {
                self.frame_cycles += 1;
                self.frame_due |= self.frame_cycles >= self.frame_period;
            }
            // A tick that arrives while an instruction is still running, or
            // while a previous play call has not returned, is held rather than
            // dropped: losing it would silently slow every tune whose play
            // routine happens to straddle the tick.
            if self.frame_due && cpu.cycles == 0 && cpu.pc == HALT {
                self.frame_due = false;
                self.frame_cycles = 0;
                cpu.pc = STUB_PLAY;
                self.frames += 1;
            }
        } else {
            // The tune owns the interrupt vector, so it is driven by its CIA the
            // same way it is on a real machine. There is no remembered request
            // here on purpose: the CIA's line is level-triggered and stays
            // asserted until the handler acknowledges it, so the line itself is
            // what has to be tested. Latching the request instead would re-fire
            // it the moment the handler returns, long after the CIA released it.
            let asserted = self.machine.borrow().cia.borrow().line();
            if asserted && cpu.cycles == 0 && (cpu.status & I_FLAG) == 0 {
                irq_entry(cpu, &mut self.machine.borrow_mut());
                self.irqs += 1;
            }
        }

        cpu.clock();
        self.machine
            .borrow_mut()
            .sid
            .render_cycles(1, &mut self.audio);
    }

    /// Read the interrupt vector back, to see whose handler is installed.
    fn irq_vector(&self) -> u16 {
        let m = self.machine.borrow();
        u16::from_le_bytes([
            m.ram[usize::from(IRQ_VECTOR)],
            m.ram[usize::from(IRQ_VECTOR + 1)],
        ])
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let opts = parse_args()?;
    let Some(path) = opts.tune else {
        return Err(
            "usage: render tune.sid [--subsong N] [--seconds S] [--ntsc] [--out file.wav]".into(),
        );
    };

    let model = if opts.ntsc { Model::Ntsc } else { Model::Pal };
    let clock_hz = model.clock_hz();

    let bytes = std::fs::read(&path)?;
    if bytes.len() < 128 || &bytes[0..4] != b"PSID" {
        return Err("not a PSID file".into());
    }
    let psid = parse_header(&bytes[..128]);
    let subsong = opts.subsong.unwrap_or(psid.start_subsong);

    let candidates = placements(&psid, &bytes);
    if candidates.is_empty() {
        return Err("the tune's data block is empty".into());
    }

    // The declared placement wins unless another one demonstrably plays.
    let mut best: Option<(u64, Placement, Rc<RefCell<Machine>>)> = None;
    for place in candidates {
        let Some((writes, machine)) = trial(model, &bytes, &psid, &place, subsong, clock_hz) else {
            continue;
        };
        let declared = best.is_none();
        if declared || writes > best.as_ref().map(|(w, _, _)| *w).unwrap_or(0) {
            best = Some((writes, place, machine));
        }
        if writes > 0 {
            break;
        }
    }
    let Some((writes, place, _)) = best else {
        return Err("no placement of the tune's block reached the chip".into());
    };

    // Re-run the winning placement from a clean chip, so the trial's half-played
    // state and its held-back audio are not part of the render.
    let machine =
        setup(model, &bytes, &psid, &place).ok_or("could not place the tune's block in memory")?;
    let (mut cpu, mut harness) = start(&machine, clock_hz, subsong);

    let total = (opts.seconds * RATE) as usize;
    let mut sink = WavSink::create(
        &opts.out,
        WavSpec {
            sample_rate: RATE as u32,
            channels: 1,
            format: SampleFormat::Pcm16,
        },
    )?;

    let mut written = 0usize;
    let mut cycles = 0u64;
    while written < total {
        harness.cycle(&mut cpu);
        cycles += 1;
        if harness.audio.len() >= 4096 {
            sink.write(&harness.audio)?;
            written += harness.audio.len();
            harness.audio.clear();
        }
    }
    if !harness.audio.is_empty() {
        written += harness.audio.len();
        sink.write(&harness.audio)?;
    }
    sink.finish()?;

    let render_writes = machine.borrow().sid_writes.get();
    println!(
        "wrote {} ({} bytes, {} samples, load ${:04X} at file {}, {} cycles, {} frames driven, {} IRQs taken, {} chip writes; the placement trial made {})",
        opts.out.display(),
        std::fs::metadata(&opts.out)?.len(),
        written,
        place.load,
        place.offset,
        cycles,
        harness.frames,
        harness.irqs,
        render_writes,
        writes,
    );
    Ok(())
}
