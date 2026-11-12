//! PSID harness: runs a tune's 6510 code and feeds the writes to the chip.
//!
//! The processor runs one Φ2 cycle at a time and the chip is rendered alongside
//! it, so a register write lands in the same cycle the tune wrote it. The tune's
//! own code owns everything it was loaded over, so the harness keeps its
//! footprint to the top of the last page: call stubs, an interrupt handler for
//! tunes that do not install one, and a halt loop, all above $ffe0.
//!
//! `cargo run --release --example render -- tune.sid [--subsong N] [--seconds S]
//!         [--ntsc] [--probe] [--out file.wav]`

use lib6502::Cpu;
use lib6502::Variant;
use lib6502::bus::Bus;
use rust_sid::sink::{SampleFormat, Sink, WavSink, WavSpec};
use rust_sid::{Model, Sid};
use std::cell::{Cell, RefCell};
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

#[path = "render/poweron_table.rs"]
mod poweron_table;

const RATE: f64 = 44_100.0;

/// Where the harness put its own three-byte and six-byte footprints. A tune
/// loaded at $E000 expects the whole top of memory as RAM, so these cannot live
/// at a fixed address; they are placed in a page the tune's own block leaves
/// free.
#[derive(Debug, Clone, Copy)]
struct Stubs {
    init: u16,
    play: u16,
    halt: u16,
    /// The stand-in for the ROM's interrupt chain, installed at the interrupt
    /// vector so a tune that enables an interrupt but brings no handler still
    /// reaches its play routine.
    irq: u16,
    /// A bare `RTS`, for a tune with no play address at all.
    nop: u16,
}

const STUB_SPAN: usize = 0x40;

/// Find a page the tune's block does not occupy, to hold the harness's stubs.
///
/// Page 2 and 3 are left alone because the tune's own zero-page variables live
/// there, and the I/O area from $D000 is left alone because it is not RAM.
fn place_stubs(load: u16, len: usize) -> Option<Stubs> {
    let lo = usize::from(load);
    let hi = lo + len;
    let mut page = 0x0400u16;
    while usize::from(page) + STUB_SPAN <= 0xd000 {
        let p = usize::from(page);
        if p + STUB_SPAN <= lo || p >= hi {
            return Some(Stubs {
                init: page,
                play: page + 8,
                halt: page + 16,
                irq: page + 24,
                nop: page + 56,
            });
        }
        page += 0x100;
    }
    None
}

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

/// The container header is 128 bytes; nothing below this is tune data.
const HEADER_LEN: usize = 128;

/// Why a container was refused. A malformed file is always an error with a named
/// reason, never a guess at a placement and never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PsidError {
    NotPsid,
    Truncated {
        data_off: usize,
        file_len: usize,
    },
    Empty,
    /// A load address that would put the tune over the zero page or the stack.
    BadLoad {
        load: u16,
    },
    /// The block would run past $CFFF into the I/O area, so it cannot be placed
    /// in the flat RAM this harness provides.
    RunsIntoIo {
        load: u16,
        len: usize,
    },
    /// The block would run past the end of the address space.
    TooLong {
        load: u16,
        len: usize,
    },
    /// The init routine is not inside the loaded block, which means the header
    /// and the data disagree.
    InitOutside {
        init: u16,
        load: u16,
        end: u16,
    },
    PlayOutside {
        play: u16,
        load: u16,
        end: u16,
    },
}

impl fmt::Display for PsidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPsid => write!(f, "not a PSID file"),
            Self::Truncated { data_off, file_len } => write!(
                f,
                "truncated: data starts at file offset {data_off} but the file is only {file_len} bytes"
            ),
            Self::Empty => write!(f, "the tune's data block is empty"),
            Self::BadLoad { load } => write!(
                f,
                "bad load address ${load:04X}: a tune cannot be loaded over the zero page and stack"
            ),
            Self::RunsIntoIo { load, len } => write!(
                f,
                "block at ${load:04X} is {len} bytes and would run into the I/O area at $D000"
            ),
            Self::TooLong { load, len } => write!(
                f,
                "block at ${load:04X} is {len} bytes and would run past the end of memory"
            ),
            Self::InitOutside { init, load, end } => write!(
                f,
                "init address ${init:04X} is outside the loaded block ${load:04X}-${end:04X}"
            ),
            Self::PlayOutside { play, load, end } => write!(
                f,
                "play address ${play:04X} is outside the loaded block ${load:04X}-${end:04X}"
            ),
        }
    }
}

impl Error for PsidError {}

/// The load-relevant fields of a PSID header. There is no data-length field: the
/// block runs from the data offset to the end of the file.
struct Psid {
    version: u16,
    /// Absolute offset into the file.
    data_off: usize,
    load: u16,
    init: u16,
    play: u16,
    songs: u8,
    /// 1-based, as the header stores it.
    start_song: u8,
    /// Per-song clock bits: a set bit means CIA 1 timer A, a clear bit means the
    /// raster interrupt.
    speed: u32,
    flags: u8,
}

/// A container resolved into something that can be placed in memory.
struct Loaded {
    load: u16,
    init: u16,
    /// Zero means the tune is interrupt-driven and must never be called.
    play: u16,
    song: u8,
    songs: u8,
    /// True when this song asks to be driven by CIA 1 timer A.
    cia_clock: bool,
    /// What the header says about the machine the tune expects.
    clock_hint: u8,
    chip_hint: u8,
    data: Vec<u8>,
}

fn parse(bytes: &[u8]) -> Result<Psid, PsidError> {
    if bytes.len() < HEADER_LEN || &bytes[0..4] != b"PSID" {
        return Err(PsidError::NotPsid);
    }
    let u16be = |o: usize| u16::from_be_bytes([bytes[o], bytes[o + 1]]);
    let u32be = |o: usize| u32::from_be_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let version = u16be(0x04);
    Ok(Psid {
        version,
        data_off: usize::from(u16be(0x06)),
        load: u16be(0x08),
        init: u16be(0x0a),
        play: u16be(0x0c),
        songs: u16be(0x0e) as u8,
        start_song: u16be(0x10) as u8,
        speed: u32be(0x12),
        // The flags field only exists from v2.
        flags: if version >= 2 { bytes[0x76] } else { 0 },
    })
}

/// Turn a header and a file into a block with a resolved load address.
///
/// A load address of zero is the normal case rather than an edge case: the block
/// carries its own address as a little-endian prefix, exactly as a program file
/// does. Some position-independent blocks carry that prefix even when the header
/// does state an address, which is recognised by the prefix naming the address
/// the block will actually be placed at.
fn resolve(psid: &Psid, bytes: &[u8], wanted: Option<u8>) -> Result<Loaded, PsidError> {
    if psid.data_off >= bytes.len() {
        return Err(PsidError::Truncated {
            data_off: psid.data_off,
            file_len: bytes.len(),
        });
    }
    let mut block = &bytes[psid.data_off..];
    if block.len() < 2 {
        return Err(PsidError::Empty);
    }

    let le16 = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
    let mut load = psid.load;
    if load == 0 {
        load = le16(block);
        block = &block[2..];
    } else if le16(block) == load.wrapping_add(2) {
        // The block carries its own address prefix even though the header gave an
        // address, so it is placed two bytes further in than the header implies.
        block = &block[2..];
    }

    let init = if psid.init == 0 { load } else { psid.init };
    // $FFFF is reserved for "not specified"; zero means the tune is driven
    // entirely by its own interrupt handler and must never be called.
    let play = if psid.play == 0xffff { 0 } else { psid.play };

    let len = block.len();
    if load < 0x0200 {
        return Err(PsidError::BadLoad { load });
    }
    if usize::from(load) + len > 0x1_0000 {
        return Err(PsidError::TooLong { load, len });
    }
    // Only the I/O window is unavailable. A block above it is legitimate: those
    // tunes disable the ROM through $01 and use $E000-$FFFF as RAM, which this
    // flat address space already is.
    let lo = usize::from(load);
    if lo < 0xe000 && lo + len > 0xd000 {
        return Err(PsidError::RunsIntoIo { load, len });
    }
    let end = load.wrapping_add(len as u16);
    if !(load..end).contains(&init) {
        return Err(PsidError::InitOutside { init, load, end });
    }
    if play != 0 && !(load..end).contains(&play) {
        return Err(PsidError::PlayOutside { play, load, end });
    }

    // Song numbering: the header is 1-based, the register the init routine reads
    // is 0-based, and both are clamped into the range the tune declares.
    let songs = if psid.songs == 0 { 1 } else { psid.songs };
    let mut song = wanted
        .map(|n| n as u16 + 1)
        .unwrap_or(u16::from(psid.start_song));
    if song < 1 || u16::from(songs) < song {
        song = 1;
    }

    Ok(Loaded {
        load,
        init,
        play,
        song: song as u8,
        songs,
        cia_clock: psid.speed & (1 << (song - 1)) != 0,
        clock_hint: (psid.flags >> 2) & 3,
        chip_hint: (psid.flags >> 4) & 3,
        data: block.to_vec(),
    })
}

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

/// A VIC-II reduced to what playing a tune needs: the raster line counter, the
/// raster-compare interrupt, and the frame clock the Kernal keeps in `$A0`.
///
/// The frame clock is not a nicety. A play routine is a slice of one frame's
/// work, and most of them decide how much of the tune to advance by measuring
/// elapsed frame clocks. With `$A0` frozen such a routine sets the chip up on
/// its first call and then repeats that same setup forever — thousands of
/// register writes and not one note — or, if it waits for the clock to change,
/// it never returns at all and the tune stops after a single frame.
#[derive(Default)]
struct Vic {
    /// `$D000..$D01F` as written. Only the raster and interrupt registers are
    /// read back with anything other than the stored value.
    regs: [u8; 0x20],
    line: u16,
    line_cycles: u64,
    lines_per_frame: u16,
    /// Set interrupt flags; bit 0 is raster.
    flags: u8,
    /// Which flags are allowed to pull the interrupt line.
    mask: u8,
}

/// Φ2 cycles per raster line on both C64 video standards.
const RASTER_LINE_CYCLES: u64 = 63;

impl Vic {
    fn new(pal: bool) -> Self {
        Self {
            lines_per_frame: if pal { 312 } else { 263 },
            ..Self::default()
        }
    }

    /// The line the raster interrupt fires on: the low eight bits from `$D012`
    /// with the ninth held by bit 3 of `$D011`.
    fn compare(&self) -> u16 {
        (u16::from(self.regs[0x11] & 0x08) << 5) | u16::from(self.regs[0x12])
    }

    /// Advance one Φ2 cycle. Returns true on the cycle a new line begins.
    fn tick(&mut self) -> bool {
        self.line_cycles += 1;
        if self.line_cycles < RASTER_LINE_CYCLES {
            return false;
        }
        self.line_cycles = 0;
        self.line = if self.line + 1 >= self.lines_per_frame {
            0
        } else {
            self.line + 1
        };
        if self.line == self.compare() {
            self.flags |= 0x01;
        }
        true
    }

    fn line(&self) -> bool {
        self.flags & self.mask & 0x01 != 0
    }

    fn read(&self, r: usize) -> u8 {
        match r {
            // Bit 7 reports the ninth bit of the *current* line; the rest is
            // whatever was written.
            0x11 => (self.regs[0x11] & 0x3f) | u8::from(self.line & 0x100 != 0) << 7,
            0x12 => (self.line & 0xff) as u8,
            0x1a => self.mask,
            _ => self.regs[r],
        }
    }

    /// Reading the interrupt register reports the flags and clears them, which
    /// is what releases the line.
    fn read_irr(&mut self) -> u8 {
        let value = self.flags | 0x80;
        self.flags = 0;
        value
    }

    fn write(&mut self, r: usize, value: u8) {
        if r == 0x19 {
            self.write_irq(value);
        } else if r == 0x1a {
            self.write_mask(value);
        } else {
            self.regs[r] = value;
        }
    }

    fn write_irq(&mut self, value: u8) {
        if value & 0x80 != 0 {
            self.flags |= value & 0x1f;
        } else {
            self.flags &= !(value & 0x1f);
        }
    }

    fn write_mask(&mut self, value: u8) {
        if value & 0x80 != 0 {
            self.mask |= value & 0x1f;
        } else {
            self.mask &= !(value & 0x1f);
        }
    }
}

struct Machine {
    ram: [u8; 0x1_0000],
    sid: Sid,
    cia: RefCell<Cia>,
    vic: RefCell<Vic>,
    /// How many writes have reached the chip. Reported in the summary as a
    /// diagnostic; it is deliberately not used to decide anything.
    sid_writes: Cell<u64>,
}

impl Machine {
    fn new(model: Model) -> Self {
        Self {
            ram: [0; 0x1_0000],
            sid: Sid::new(model, RATE),
            cia: RefCell::new(Cia::default()),
            vic: RefCell::new(Vic::new(model == Model::Pal)),
            sid_writes: Cell::new(0),
        }
    }

    /// Advance the two interrupt sources by one Φ2 cycle, and tick the frame
    /// clock the Kernal keeps in `$A0` once per raster line. Returns the cycle on
    /// which the CIA's line rises, which is what drives a tune played on the
    /// CIA rather than on the harness's own counter.
    fn tick(&mut self) -> bool {
        let underflow = self.cia.borrow_mut().tick();
        if self.vic.borrow_mut().tick() {
            self.ram[0x00a0] = self.ram[0x00a0].wrapping_add(1);
        }
        underflow
    }

    /// Whether either chip is currently pulling the processor's interrupt line.
    fn irq_line(&self) -> bool {
        self.cia.borrow().line() || self.vic.borrow().line()
    }

    /// Whether the tune has armed an interrupt source at all.
    ///
    /// A CIA whose latch is still zero would underflow on every cycle and turn
    /// the interrupt into a storm, so an armed-but-unlatched timer counts as
    /// disarmed and the tune is played on the harness's own frame counter
    /// instead.
    fn irq_sources_enabled(&self) -> bool {
        let cia = self.cia.borrow();
        let armed = cia.started && cia.latch != 0;
        armed || self.vic.borrow().mask != 0
    }
}

impl Bus for Machine {
    fn read(&self, address: u16) -> u8 {
        let a = usize::from(address);
        match a {
            0xd400..=0xd5ff => self.sid.read(a - 0xd400),
            0xd000..=0xd01f => match a - 0xd000 {
                0x19 => self.vic.borrow_mut().read_irr(),
                r => self.vic.borrow().read(r),
            },
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
            0xd000..=0xd01f => self.vic.borrow_mut().write(a - 0xd000, value),
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

/// `JSR <entry>` then `JMP halt`, so the tune's `RTS` brings us back to the stub
/// and the stub lands us on the halt loop. Using the processor's own `JSR` means
/// the stack frame is laid out exactly as the tune expects it.
fn write_call_stub(ram: &mut [u8; 0x1_0000], stub: u16, entry: u16, halt: u16) {
    let s = usize::from(stub);
    ram[s] = 0x20; // JSR absolute
    ram[s + 1] = (entry & 0xff) as u8;
    ram[s + 2] = (entry >> 8) as u8;
    ram[s + 3] = 0x4c; // JMP absolute
    ram[s + 4] = (halt & 0xff) as u8;
    ram[s + 5] = (halt >> 8) as u8;
}

/// `JSR play` then `JMP halt`, for a tune that leaves the interrupt vector alone.
fn write_play_stub(ram: &mut [u8; 0x1_0000], stubs: Stubs, play: u16) {
    write_call_stub(ram, stubs.play, play, stubs.halt);
}

/// `JMP halt`, so an idle processor stays put instead of executing whatever the
/// zeros in memory would have it run.
fn write_halt_loop(ram: &mut [u8; 0x1_0000], halt: u16) {
    let s = usize::from(halt);
    ram[s] = 0x4c;
    ram[s + 1] = (halt & 0xff) as u8;
    ram[s + 2] = (halt >> 8) as u8;
}

/// A stand-in for the ROM's interrupt chain, for the large class of tunes that
/// enable a hardware interrupt in their init but bring no handler of their own,
/// expecting the machine's existing one to reach their play routine.
///
/// Without it such a tune waits forever on a flag only its own handler would
/// set: the init returns, the harness calls play once, and play never comes back.
/// Saving and restoring the registers around the call is what lets the
/// processor return to whatever it was doing.
fn write_irq_driver(ram: &mut [u8; 0x1_0000], stubs: Stubs, play: u16) {
    let s = usize::from(stubs.irq);
    let entry = if play == 0 { stubs.nop } else { play };
    let code: [u8; 20] = [
        0x48, // PHA
        0x8a, // TXA
        0x48, // PHA
        0x98, // TYA
        0x48, // PHA
        0xad,
        0x0d,
        0xdc, // LDA $DC0D — acknowledge and release the CIA's line
        0xad,
        0x19,
        0xd0, // LDA $D019 — acknowledge and release the VIC's line
        0x20,
        (entry & 0xff) as u8,
        (entry >> 8) as u8, // JSR play
        0x68,               // PLA
        0xa8,               // TAY
        0x68,               // PLA
        0xaa,               // TAX
        0x68,               // PLA
        0x40,               // RTI
    ];
    ram[s..s + code.len()].copy_from_slice(&code);
    ram[usize::from(stubs.nop)] = 0x60; // RTS
}

/// Lay down the machine state a tune expects to find, before its init runs.
fn write_machine_state(ram: &mut [u8; 0x1_0000], pal: bool, load: u16, end: u16) {
    for (addr, value) in poweron_table::POWERON.iter().enumerate() {
        ram[addr] = *value;
    }
    // The video-standard flag the Kernal keeps in $02A6: 1 for PAL, 0 for NTSC.
    ram[0x02a6] = u8::from(pal);
    // What the Kernal's LOAD vector leaves behind when a file is loaded, because
    // tunes that use BASIC variables or examine the text pointers depend on it.
    for addr in [0x002d, 0x002f, 0x0031, 0x00ae] {
        ram[addr] = (end & 0xff) as u8;
        ram[addr + 1] = (end >> 8) as u8;
    }
    ram[0x00ac] = (load & 0xff) as u8;
    ram[0x00ad] = (load >> 8) as u8;
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
    /// 0-based, as the register the init routine reads is. Left unset to mean
    /// "whatever the tune's own header asks to start on".
    subsong: Option<u8>,
    seconds: f64,
    ntsc: bool,
    out: PathBuf,
    /// Report the resolved header and stop, without rendering.
    probe: bool,
}

fn parse_args() -> Result<Options, Box<dyn Error>> {
    let mut o = Options {
        tune: None,
        subsong: None,
        seconds: 30.0,
        ntsc: false,
        out: PathBuf::from("out.wav"),
        probe: false,
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
            "--probe" => o.probe = true,
            other => o.tune = Some(PathBuf::from(other)),
        }
    }

    Ok(o)
}

/// Place the resolved block in memory and lay down the harness's own stubs.
fn setup(model: Model, loaded: &Loaded, pal: bool, stubs: Stubs) -> Rc<RefCell<Machine>> {
    let machine = Rc::new(RefCell::new(Machine::new(model)));
    {
        let mut m = machine.borrow_mut();
        write_machine_state(
            &mut m.ram,
            pal,
            loaded.load,
            loaded.load + loaded.data.len() as u16,
        );
        let load = usize::from(loaded.load);
        m.ram[load..load + loaded.data.len()].copy_from_slice(&loaded.data);
        write_call_stub(&mut m.ram, stubs.init, loaded.init, stubs.halt);
        if loaded.play != 0 {
            write_play_stub(&mut m.ram, stubs, loaded.play);
        }
        write_halt_loop(&mut m.ram, stubs.halt);
        write_irq_driver(&mut m.ram, stubs, loaded.play);
        // Point the interrupt vector at the harness's stand-in for the ROM's
        // chain. A tune that installs its own handler overwrites this and owns
        // its own timing; one that leaves it alone still reaches its play
        // routine when a hardware interrupt fires.
        m.ram[usize::from(IRQ_VECTOR)] = (stubs.irq & 0xff) as u8;
        m.ram[usize::from(IRQ_VECTOR + 1)] = (stubs.irq >> 8) as u8;
        m.ram[usize::from(RESET_VECTOR)] = (stubs.halt & 0xff) as u8;
        m.ram[usize::from(RESET_VECTOR + 1)] = (stubs.halt >> 8) as u8;
    }
    machine
}

/// Start a processor on a prepared machine and run its init to completion.
fn start(machine: &Rc<RefCell<Machine>>, stubs: Stubs, clock_hz: f64, song: u8) -> (Cpu, Harness) {
    let mut cpu = Cpu::new(Box::new(BusHandle(Rc::clone(machine))), 0);
    cpu.change_variant(Variant::NMOS);
    cpu.set_illegal_opcodes(true);
    cpu.reset();
    // The PSID convention passes the starting subsong to the init routine in A,
    // 0-based. Init is entered with interrupts disabled, as on a real machine;
    // a frame clock that waited for the processor's own I flag to be clear would
    // never fire for a tune that never re-enables them.
    cpu.a = song - 1;
    cpu.status |= I_FLAG;
    cpu.pc = stubs.init;

    let mut harness = Harness::new(Rc::clone(machine), clock_hz, stubs);
    let mut budget = 20_000_000u64;
    while cpu.pc != stubs.halt && budget > 0 {
        harness.cycle(&mut cpu);
        budget -= 1;
    }
    // Who drives the frame. A tune that installed its own handler owns its own
    // timing and must only ever be reached by its interrupts. One that left the
    // vector alone but armed a hardware interrupt is reached through the
    // harness's stand-in for the ROM's chain. One that armed nothing at all is
    // played on the harness's own counter, which is the usual case for a tune
    // that simply expects to be called once a frame.
    let owns_vector = harness.irq_vector() != stubs.irq;
    let armed = harness.machine.borrow().irq_sources_enabled();
    harness.frame_clock = !owns_vector && harness.has_play_stub && !armed;
    // Every interrupt that reaches the tune through the harness's driver is one
    // frame of the tune, so the summary can still report a frame count.
    harness.frames_by_driver = !owns_vector && !harness.frame_clock;
    (cpu, harness)
}

/// Everything the cycle loop carries between iterations.
struct Harness {
    machine: Rc<RefCell<Machine>>,
    stubs: Stubs,
    /// False for a tune with no play address, which must never be called.
    has_play_stub: bool,
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
    /// The tune is reached through the harness's stand-in for the ROM's chain, so
    /// each interrupt delivered is one frame of the tune.
    frames_by_driver: bool,
    audio: Vec<f32>,
}

impl Harness {
    fn new(machine: Rc<RefCell<Machine>>, clock_hz: f64, stubs: Stubs) -> Self {
        let has_play_stub = {
            let m = machine.borrow();
            m.ram[usize::from(stubs.play)] == 0x20
        };
        Self {
            machine,
            stubs,
            has_play_stub,
            frame_cycles: 0,
            frame_due: false,
            // Overridden once the tune's own clock preference is known.
            frame_period: (clock_hz / 50.0) as u64,
            frame_clock: false,
            irqs: 0,
            frames: 0,
            frames_by_driver: false,
            audio: Vec::with_capacity(4096),
        }
    }

    /// A tune that asked for CIA 1 timer A but never programmed one still has to
    /// be played; the CIA's nominal rate is 60 Hz, which is what the raster
    /// interrupt stands in for on a 50 Hz machine.
    fn set_frame_clock(&mut self, clock_hz: f64, cia_requested: bool, pal: bool) {
        self.frame_period = (clock_hz / if cia_requested || !pal { 60.0 } else { 50.0 }) as u64;
    }

    /// One Φ2 cycle: advance the CIA, offer any pending interrupt at an
    /// instruction boundary, step the processor, then let the chip run the cycle
    /// it just saw. An instruction in progress always finishes, which is why the
    /// request is only offered when the processor has no cycles left to spend.
    fn cycle(&mut self, cpu: &mut Cpu) {
        let underflow = self.machine.borrow_mut().tick();
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
            if self.frame_due && cpu.cycles == 0 && cpu.pc == self.stubs.halt {
                self.frame_due = false;
                self.frame_cycles = 0;
                cpu.pc = self.stubs.play;
                self.frames += 1;
            }
        } else {
            // The tune owns the interrupt vector, so it is driven by its own
            // chips the same way it is on a real machine. There is no remembered
            // request here on purpose: both interrupt lines are level-triggered
            // and stay asserted until the handler acknowledges them, so the line
            // itself is what has to be tested. Latching the request instead would
            // re-fire it the moment the handler returns, long after the chip
            // released it.
            let asserted = self.machine.borrow().irq_line();
            if asserted && cpu.cycles == 0 && (cpu.status & I_FLAG) == 0 {
                irq_entry(cpu, &mut self.machine.borrow_mut());
                self.irqs += 1;
                if self.frames_by_driver {
                    self.frames += 1;
                }
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

fn main() -> std::process::ExitCode {
    // A named refusal has to reach the reader as the sentence that explains it,
    // not as the debug form of the variant.
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let opts = parse_args()?;
    let Some(path) = opts.tune else {
        return Err(
            "usage: render tune.sid [--subsong N] [--seconds S] [--ntsc] [--probe] [--out file.wav]"
                .into(),
        );
    };

    let bytes = std::fs::read(&path)?;
    let psid = parse(&bytes)?;
    let loaded = resolve(&psid, &bytes, opts.subsong)?;

    // The header's own preference decides unless the caller overrode it.
    let pal = match (opts.ntsc, psid.version, loaded.clock_hint) {
        (true, _, _) => false,
        (false, 2.., 1) => true,
        (false, 2.., 2) => false,
        _ => true,
    };
    let model = if pal { Model::Pal } else { Model::Ntsc };
    let clock_hz = model.clock_hz();

    let stubs = place_stubs(loaded.load, loaded.data.len()).ok_or(
        "the tune's block occupies every free page, leaving nowhere for the harness's call stubs",
    )?;

    if opts.probe {
        println!(
            "v{} song {}/{} load ${:04X} init ${:04X} play ${:04X} ({}), {} bytes at file {}, stubs ${:04X}, {} clock, {} chip, {} CIA clock",
            psid.version,
            loaded.song,
            loaded.songs,
            loaded.load,
            loaded.init,
            loaded.play,
            if loaded.play == 0 {
                "interrupt-driven"
            } else {
                "called"
            },
            loaded.data.len(),
            psid.data_off,
            stubs.init,
            if pal { "PAL" } else { "NTSC" },
            loaded.chip_hint,
            loaded.cia_clock,
        );
        return Ok(());
    }

    let machine = setup(model, &loaded, pal, stubs);
    let (mut cpu, mut harness) = start(&machine, stubs, clock_hz, loaded.song);
    harness.set_frame_clock(clock_hz, loaded.cia_clock, pal);

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
    // The two registers that decide whether anything can be heard: the master
    // volume and filter mode, and voice 1's waveform and gate.
    let (mode_vol, v1_ctrl, v1_freq) = {
        let m = machine.borrow();
        (
            m.sid.read(24),
            m.sid.read(4),
            u16::from_le_bytes([m.sid.read(0), m.sid.read(1)]),
        )
    };
    println!(
        "wrote {} ({} bytes, {} samples, load ${:04X}, init ${:04X}, play ${:04X}, song {}/{}, {} cycles, {} frames driven, {} IRQs taken, {} chip writes, final $d418=${:02X} $d404=${:02X} freq={})",
        opts.out.display(),
        std::fs::metadata(&opts.out)?.len(),
        written,
        loaded.load,
        loaded.init,
        loaded.play,
        loaded.song,
        loaded.songs,
        cycles,
        harness.frames,
        harness.irqs,
        render_writes,
        mode_vol,
        v1_ctrl,
        v1_freq,
    );

    Ok(())
}
