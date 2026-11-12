//! PSID harness: runs a tune's 6510 code and feeds the writes to the chip.
//!
//! The processor runs one Φ2 cycle at a time and the chip is rendered alongside
//! it, so a register write lands in the same cycle the tune wrote it. The tune's
//! own code owns everything it was loaded over, so the harness keeps its
//! footprint to the top of the last page: call stubs, an interrupt handler for
//! tunes that do not install one, and a halt loop, all above $ffe0.
//!
//! `cargo run --release -p sid-play --bin render -- tune.sid [--subsong N] [--seconds S]
//!         [--ntsc] [--probe] [--out file.wav]`

use mos6510::{Bus, Cpu};
use mos6581::{Model, Sid};
use sid_play::sink::{SampleFormat, Sink, WavSink, WavSpec};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

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
}

const STUB_SPAN: usize = 0x20;

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
            });
        }
        page += 0x100;
    }
    None
}

const IRQ_VECTOR: u16 = 0xfffe;
const RESET_VECTOR: u16 = 0xfffc;

/// The status register's interrupt-disable bit.
const I_FLAG: u8 = 0x04;
/// The 6510 port as the Kernal leaves it: `$00` (direction) = `$2F`, `$01`
/// (data) = `$37`. The input pins read the cassette sense line high and the
/// unconnected bits 6-7 long faded to 0, so `$01` reads back `$37`.
const PORT_DDR: u8 = 0x2f;
const PORT_DATA: u8 = 0x37;
const PORT_PINS: u8 = 0x10;
/// The two ROM areas. With no ROM image installed these read as zeros, so a tune
/// that reaches them executes nothing.
const ROM_BASIC: (u16, u16) = (0xa000, 0xbfff);
const ROM_KERNAL: (u16, u16) = (0xe000, 0xffff);
/// The RAM copy of the interrupt vectors a tune may rewrite, and the vectors
/// themselves.
const VEC_RAM: (u16, u16) = (0x0314, 0x0319);
const VEC_ROM: (u16, u16) = (0xfffa, 0xffff);
/// How many distinct sites `--trace` remembers before it stops widening its
/// tables. Counts for sites already recorded keep accumulating.
const TRACE_LIMIT: usize = 400;

fn in_range(addr: u16, range: (u16, u16)) -> bool {
    addr >= range.0 && addr <= range.1
}

/// Which half of the tune's life a recorded cycle belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Init,
    Run,
}

/// Where the tune actually went, recorded only when `--trace` is given.
///
/// The point is attribution: a tune that produces nothing is either calling into
/// a ROM area that holds no ROM, or it is not calling what we think it is. Those
/// need different fixes, and the only way to tell them apart is to record the
/// addresses rather than reason about them.
#[derive(Debug)]
struct Trace {
    enabled: bool,
    phase: Phase,
    /// The last program counter seen, so a jump into a ROM area can be traced
    /// back to the instruction that made it.
    prev_pc: u16,
    rom_exec: BTreeMap<u16, u64>,
    /// (caller, callee) pairs for transfers into a ROM area.
    rom_edges: BTreeMap<(u16, u16), u64>,
    /// Whether the byte at that ROM address was zero: a real entry point has an
    /// opcode there, a tune that wandered off has nothing.
    rom_zero: BTreeMap<u16, bool>,
    /// Where time went outside the ROM areas, to localise a play routine that
    /// never returns.
    run_pc: BTreeMap<u16, u64>,
    init_cycles: u64,
    init_returned: bool,
}

impl Trace {
    fn new() -> Self {
        Self {
            enabled: false,
            phase: Phase::Init,
            prev_pc: 0,
            rom_exec: BTreeMap::new(),
            rom_edges: BTreeMap::new(),
            rom_zero: BTreeMap::new(),
            run_pc: BTreeMap::new(),
            init_cycles: 0,
            init_returned: false,
        }
    }

    fn note(&mut self, pc: u16, boundary: bool, ram: &[u8; 0x1_0000]) {
        if !self.enabled {
            return;
        }
        if self.phase == Phase::Init {
            self.init_cycles += 1;
        }
        // Operand fetches advance the program counter too; only the first
        // cycle of an instruction names an instruction site.
        if !boundary {
            return;
        }
        if in_range(pc, ROM_BASIC) || in_range(pc, ROM_KERNAL) {
            *self.rom_exec.entry(pc).or_insert(0) += 1;
            if self.rom_exec.len() <= TRACE_LIMIT {
                *self.rom_edges.entry((self.prev_pc, pc)).or_insert(0) += 1;
                self.rom_zero.insert(pc, ram[usize::from(pc)] == 0);
            }
        } else if self.phase == Phase::Run && self.run_pc.len() <= TRACE_LIMIT {
            *self.run_pc.entry(pc).or_insert(0) += 1;
        }
        self.prev_pc = pc;
    }

    /// The `n` entries with the highest counts, highest first.
    fn top_v(&self, map: &BTreeMap<u16, u64>, n: usize) -> Vec<(u16, u64)> {
        let mut v: Vec<(u16, u64)> = map.iter().map(|(k, c)| (*k, *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    fn top_k(&self, n: usize) -> Vec<((u16, u16), u64)> {
        let mut v: Vec<((u16, u16), u64)> = self.rom_edges.iter().map(|(k, c)| (*k, *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    fn rom_total(&self, range: (u16, u16)) -> u64 {
        self.rom_exec
            .iter()
            .filter(|(pc, _)| in_range(**pc, range))
            .map(|(_, c)| c)
            .sum()
    }
}

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

struct Machine {
    ram: [u8; 0x1_0000],
    sid: Sid,
    cia: RefCell<Cia>,
    /// How many writes have reached the chip. Reported in the summary as a
    /// diagnostic; it is deliberately not used to decide anything.
    sid_writes: Cell<u64>,
    /// `--trace` only: reads from the ROM areas, and rewrites of the interrupt
    /// vectors. Both stay empty while tracing is off.
    trace: Cell<bool>,
    rom_reads: RefCell<BTreeMap<u16, u64>>,
    vector_writes: RefCell<BTreeMap<(u16, u16), u64>>,
    /// Writes per register offset into the chip's 32-register file, and a count
    /// of those that fell outside it. A tune writing past register 31 is being
    /// silently ignored, and the totals alone cannot show that.
    sid_reg_writes: RefCell<[u64; 32]>,
    sid_oob_writes: Cell<u64>,
    /// `--trace` only: every address read after init finished, so the
    /// memory a play routine depends on can be compared against what
    /// was actually loaded. Reads of untouched RAM are what to look for.
    play_reads: RefCell<BTreeMap<u16, u64>>,
    running: Cell<bool>,
    /// Where the tune's own block landed, so a read can be judged inside
    /// or outside it.
    block: Cell<(u16, u16)>,
}

impl Machine {
    fn new(model: Model) -> Self {
        Self {
            ram: [0; 0x1_0000],
            sid: Sid::new(model, RATE),
            cia: RefCell::new(Cia::default()),
            sid_writes: Cell::new(0),
            trace: Cell::new(false),
            rom_reads: RefCell::new(BTreeMap::new()),
            vector_writes: RefCell::new(BTreeMap::new()),
            sid_reg_writes: RefCell::new([0; 32]),
            sid_oob_writes: Cell::new(0),
            play_reads: RefCell::new(BTreeMap::new()),
            running: Cell::new(false),
            block: Cell::new((0, 0)),
        }
    }
}

impl Machine {
    fn read(&self, address: u16) -> u8 {
        let a = usize::from(address);
        if self.trace.get() && (in_range(address, ROM_BASIC) || in_range(address, ROM_KERNAL)) {
            *self.rom_reads.borrow_mut().entry(address).or_insert(0) += 1;
        }
        if self.trace.get() && self.running.get() && self.play_reads.borrow().len() < 4000 {
            *self.play_reads.borrow_mut().entry(address).or_insert(0) += 1;
        }
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
        if self.trace.get()
            && (in_range(address, VEC_RAM) || in_range(address, VEC_ROM))
            && self.vector_writes.borrow().len() <= TRACE_LIMIT
        {
            *self
                .vector_writes
                .borrow_mut()
                .entry((address, u16::from(value)))
                .or_insert(0) += 1;
        }
        match a {
            0xd400..=0xd5ff => {
                self.sid_writes.set(self.sid_writes.get() + 1);
                let reg = a - 0xd400;
                if self.trace.get() {
                    // `Sid::write` ignores anything past register 31, so a tune
                    // reaching the mirrored or extended addresses is being
                    // discarded. The totals alone cannot show that.
                    if reg < 32 {
                        self.sid_reg_writes.borrow_mut()[reg] += 1;
                    } else {
                        self.sid_oob_writes.set(self.sid_oob_writes.get() + 1);
                    }
                }
                self.sid.write(reg, value);
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
}

/// The bus handle the processor holds. The CPU owns its bus, so this is how the
/// harness keeps reaching the same chip and memory the tune is driving.
struct BusHandle(Rc<RefCell<Machine>>);

impl Bus for BusHandle {
    fn read(&mut self, address: u16) -> u8 {
        self.0.borrow().read(address)
    }

    fn write(&mut self, address: u16, value: u8) {
        self.0.borrow_mut().write(address, value);
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
    /// Record where the tune executes. Inert unless asked for.
    trace: bool,
}

fn parse_args() -> Result<Options, Box<dyn Error>> {
    let mut o = Options {
        tune: None,
        subsong: None,
        seconds: 30.0,
        ntsc: false,
        out: PathBuf::from("out.wav"),
        probe: false,
        trace: false,
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
            "--trace" => o.trace = true,
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
        m.block
            .set((loaded.load, loaded.load + loaded.data.len() as u16));
        write_call_stub(&mut m.ram, stubs.init, loaded.init, stubs.halt);
        if loaded.play != 0 {
            write_play_stub(&mut m.ram, stubs, loaded.play);
        }
        write_halt_loop(&mut m.ram, stubs.halt);
        // Point the interrupt vector at the halt loop, so a tune that never
        // installs a handler leaves it there and the harness can tell. A tune
        // that does install one overwrites this and owns its own timing.
        m.ram[usize::from(IRQ_VECTOR)] = (stubs.halt & 0xff) as u8;
        m.ram[usize::from(IRQ_VECTOR + 1)] = (stubs.halt >> 8) as u8;
        m.ram[usize::from(RESET_VECTOR)] = (stubs.halt & 0xff) as u8;
        m.ram[usize::from(RESET_VECTOR + 1)] = (stubs.halt >> 8) as u8;
    }
    machine
}

/// Start a processor on a prepared machine and run its init to completion.
fn start(machine: &Rc<RefCell<Machine>>, stubs: Stubs, clock_hz: f64, song: u8) -> (Cpu, Harness) {
    let mut cpu = Cpu::new(mos6510::Model::Mos6510);
    cpu.set_port_registers(PORT_DDR, PORT_DATA);
    cpu.set_port_input(PORT_PINS);
    cpu.s = 0xfd;
    // The PSID convention passes the starting subsong to the init routine in A,
    // 0-based. Init is entered with interrupts disabled, as on a real machine;
    // a frame clock that waited for the processor's own I flag to be clear would
    // never fire for a tune that never re-enables them.
    cpu.a = song - 1;
    cpu.p |= I_FLAG;
    cpu.jump(stubs.init);

    let mut harness = Harness::new(Rc::clone(machine), clock_hz, stubs);
    let mut budget = 20_000_000u64;
    // An IRQ through the default halt vector mid-init also lands on the halt
    // loop, but with S below $FD; only a real return restores it.
    let returned =
        |cpu: &Cpu| cpu.at_instruction_boundary() && cpu.pc == stubs.halt && cpu.s == 0xfd;
    while !returned(&cpu) && !cpu.jammed() && budget > 0 {
        harness.cycle(&mut cpu);
        budget -= 1;
    }
    // Whose handler is installed decides who drives the frame. A tune with no
    // play address at all is driven purely by its own interrupts.
    harness.frame_clock = harness.has_play_stub && harness.irq_vector() == stubs.halt;
    let init_returned = returned(&cpu) && !cpu.jammed();
    if harness.frame_clock {
        // Clears an interrupt decision latched under the CIA line before the harness took over.
        cpu.jump(stubs.halt);
    }
    harness.trace.init_returned = init_returned;
    harness.trace.phase = Phase::Run;
    harness.machine.borrow_mut().running.set(true);
    (cpu, harness)
}

/// Everything the cycle loop carries between iterations.
struct Harness {
    /// The processor's view of the machine, ticked once per cycle.
    bus: BusHandle,
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
    audio: Vec<f32>,
    /// `--trace` only.
    trace: Trace,
}

impl Harness {
    fn new(machine: Rc<RefCell<Machine>>, clock_hz: f64, stubs: Stubs) -> Self {
        let has_play_stub = {
            let m = machine.borrow();
            m.ram[usize::from(stubs.play)] == 0x20
        };
        let tracing = machine.borrow().trace.get();
        Self {
            bus: BusHandle(Rc::clone(&machine)),
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
            audio: Vec::with_capacity(4096),
            trace: Trace {
                enabled: tracing,
                ..Trace::new()
            },
        }
    }

    /// A tune that asked for CIA 1 timer A but never programmed one still has to
    /// be played; the CIA's nominal rate is 60 Hz, which is what the raster
    /// interrupt stands in for on a 50 Hz machine.
    fn set_frame_clock(&mut self, clock_hz: f64, cia_requested: bool, pal: bool) {
        self.frame_period = (clock_hz / if cia_requested || !pal { 60.0 } else { 50.0 }) as u64;
    }

    /// One Φ2 cycle: advance the CIA, then either drive the play stub (frame
    /// clock) or wire the CIA's line to the processor's IRQ pin. Count an
    /// interrupt the processor is about to take, tick it one cycle, and render
    /// the chip for that cycle.
    fn cycle(&mut self, cpu: &mut Cpu) {
        if self.trace.enabled {
            let pc = cpu.pc;
            let m = self.machine.borrow();
            self.trace.note(pc, cpu.at_instruction_boundary(), &m.ram);
        }
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
            if self.frame_due && cpu.at_instruction_boundary() && cpu.pc == self.stubs.halt {
                self.frame_due = false;
                self.frame_cycles = 0;
                cpu.jump(self.stubs.play);
                self.frames += 1;
            }
            // The harness, not the CIA, drives the player here; release any
            // level latched from the last init cycle.
            cpu.set_irq(false);
        } else {
            // The tune owns the interrupt vector, so it is driven by its CIA the
            // same way it is on a real machine: the CIA's level-triggered line
            // is wired to the processor's, which samples it where the hardware
            // does.
            cpu.set_irq(self.machine.borrow().cia.borrow().line());
        }
        // Counted at the boundary, so the summary reports interrupts the
        // processor actually took rather than ones merely requested.
        if cpu.interrupt_pending() {
            self.irqs += 1;
        }
        cpu.tick(&mut self.bus);
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
    if opts.trace {
        machine.borrow().trace.set(true);
    }
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
    // Only kept under `--trace`, to report what the render actually sounds like
    // alongside where it went.
    let mut heard: Vec<f32> = Vec::new();
    while written < total {
        harness.cycle(&mut cpu);
        cycles += 1;
        if harness.audio.len() >= 4096 {
            if opts.trace {
                heard.extend_from_slice(&harness.audio);
            }
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

    if opts.trace {
        let rms = |v: &[f32]| -> f64 {
            if v.is_empty() {
                0.0
            } else {
                (v.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / v.len() as f64)
                    .sqrt()
            }
        };
        let quarter = heard.len() / 4;
        let lastq = if quarter > 0 {
            rms(&heard[3 * quarter..])
        } else {
            0.0
        };
        let nz = if heard.is_empty() {
            0.0
        } else {
            heard.iter().filter(|&&s| s != 0.0).count() as f64 / heard.len() as f64
        };
        let m = machine.borrow();
        let t = &harness.trace;
        // Every register that can silence a voice: the three waveform/gate
        // controls, the filter routing, and the master volume with its taps.
        let (v2_ctrl, v3_ctrl, routing) = (m.sid.read(11), m.sid.read(18), m.sid.read(23));
        let regw: String = m
            .sid_reg_writes
            .borrow()
            .iter()
            .enumerate()
            .filter(|&(_, &c)| c > 0)
            .map(|(r, &c)| format!("{r}:{c}"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "TRACE heard={:.4} lastq={:.4} nz={:.2} init_returned={} init_cycles={} romexec_basic={} romexec_kernal={} romreads={} vecwrites={} irqs={} frames={} writes={} oob={} regw={} d418=${:02X} d417=${:02X} d404=${:02X} d40b=${:02X} d412=${:02X} freq={}",
            rms(&heard),
            lastq,
            nz,
            u8::from(t.init_returned),
            t.init_cycles,
            t.rom_total(ROM_BASIC),
            t.rom_total(ROM_KERNAL),
            m.rom_reads.borrow().values().sum::<u64>(),
            m.vector_writes.borrow().values().sum::<u64>(),
            harness.irqs,
            harness.frames,
            render_writes,
            m.sid_oob_writes.get(),
            regw,
            mode_vol,
            routing,
            v1_ctrl,
            v2_ctrl,
            v3_ctrl,
            v1_freq,
        );
        let (b0, b1) = m.block.get();
        let mut reads: Vec<(u16, u64)> = m
            .play_reads
            .borrow()
            .iter()
            .map(|(&a, &c)| (a, c))
            .collect();
        reads.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
        for (a, c) in reads.iter().take(12) {
            let inside = *a >= b0 && *a < b1;
            println!(
                "TRACE playread ${a:04X} n={c} in_block={}",
                u8::from(inside)
            );
        }
        for (pc, c) in t.top_v(&t.rom_exec, 5) {
            println!(
                "TRACE exec ${pc:04X} n={c} zero={}",
                u8::from(t.rom_zero.get(&pc).copied().unwrap_or(true))
            );
        }
        for ((from, to), c) in t.top_k(5) {
            println!("TRACE edge ${from:04X}->${to:04X} n={c}");
        }
        let mut reads: Vec<(u16, u64)> =
            m.rom_reads.borrow().iter().map(|(k, v)| (*k, *v)).collect();
        reads.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (a, c) in reads.into_iter().take(5) {
            println!("TRACE romread ${a:04X} n={c}");
        }
        for (pc, c) in t.top_v(&t.run_pc, 5) {
            println!("TRACE runpc ${pc:04X} n={c}");
        }
        let mut vecs: Vec<((u16, u16), u64)> = m
            .vector_writes
            .borrow()
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        vecs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for ((a, v), c) in vecs.into_iter().take(8) {
            println!("TRACE vecwrite ${a:04X}=${v:02X} n={c}");
        }
    }
    Ok(())
}
