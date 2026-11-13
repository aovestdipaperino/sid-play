//! PSID harness: runs a tune's 6510 code and feeds the writes to the chip.
//!
//! The processor runs one Φ2 cycle at a time and the chip is rendered alongside
//! it, so a register write lands in the same cycle the tune wrote it. The tune's
//! own code owns everything it was loaded over, so the harness keeps its
//! footprint small and out of the tune's way: call stubs, a halt loop and a
//! KERNAL-like interrupt chain in one page the tune's block leaves free, plus
//! the KERNAL's own chain entry points at $EA31 and $EA7E-$EA86 when the
//! block does not cover them.
//!
//! `cargo run --release -p sid-play --bin render -- tune.sid [--subsong N] [--seconds S]
//!         [--songlengths FILE] [--ntsc] [--probe] [--poweron FILE] [--sink wav|audio]
//!         [--out file.wav] [--quiet]`
//!
//! Without `--seconds` the tune plays to its end: the length HVSC's
//! `Songlengths.md5` gives for it (found above the tune, or named with
//! `--songlengths`), or else until it has been silent for a few seconds, capped
//! at ten minutes. On a terminal, a progress bar and a spectrum analyser drawn
//! in braille run on stderr; `--quiet` turns them off.
//!
//! `--sink audio` plays through the default output device and needs a build
//! with `--features playback`; `--out` applies only to `--sink wav`.

use mos6510::{Bus, Cpu};
use mos6581::{Model, Sid};
use sid_play::display::{Length, Screen};
use sid_play::sink::{SampleFormat, Sink, WavSink, WavSpec};
use sid_play::songlength;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::rc::Rc;

const RATE: f64 = 44_100.0;

/// With no known length, the tune ends once it has been this quiet for this
/// long after making a sound...
const SILENCE_SECONDS: f64 = 3.0;
/// ...or this long if it never made one.
const NEVER_HEARD_SECONDS: f64 = 10.0;
/// The quietest sample peak that still counts as sound: about -54 dBFS.
const SILENCE_LEVEL: f32 = 0.002;
/// The longest an unknown-length tune plays; most loop forever.
const MAX_SECONDS: f64 = 600.0;

/// Where the harness put its own small routines. A tune loaded at $E000
/// expects the whole top of memory as RAM, so these cannot live at a fixed
/// address; they are placed in a page the tune's own block leaves free.
#[derive(Debug, Clone, Copy)]
struct Stubs {
    init: u16,
    play: u16,
    halt: u16,
    /// Where the hardware IRQ vector points: save A, X and Y, then
    /// `JMP ($0314)`, as the KERNAL's entry at $FF48 does.
    entry: u16,
    /// Where $0314 points by default: acknowledge CIA 1, restore the registers
    /// and return, as the KERNAL's handler ends ($EA7E-$EA86).
    ack: u16,
    /// Where the hardware NMI vector points: `SEI / JMP ($0318)`, as the
    /// KERNAL's NMI entry at $FE43 begins.
    nmi_entry: u16,
    /// Where $0318 points by default: acknowledge CIA 2 and return.
    nmi_ack: u16,
    /// True when the tune's block leaves $EA31-$EA86 free, so the KERNAL's
    /// own chain entry points can be laid down there for handlers that end
    /// with `JMP $EA31` or `JMP $EA81`.
    kernal_chain: bool,
}

/// Eight slots of 8 bytes: init and play call stubs (6 bytes each), the halt
/// loop (3), the IRQ entry (8), the default IRQ handler (9, so it spills a
/// byte into the sixth slot), the NMI entry (4) and the default NMI handler
/// (6).
const STUB_SPAN: usize = 0x40;

/// The KERNAL's IRQ handler entry ($EA31) and the part of it that acknowledges
/// CIA 1 and restores the registers ($EA7E-$EA86). See [`write_kernal_chain`].
const KERNAL_IRQ: u16 = 0xea31;
const KERNAL_IRQ_END: u16 = 0xea7e;
/// $0314-$0315, CINV: the RAM vector the KERNAL's IRQ entry jumps through.
const CINV: u16 = 0x0314;

/// Find a page the tune's block does not occupy, to hold the harness's stubs.
///
/// Page 2 and 3 are left alone because the tune's own zero-page variables live
/// there, and the I/O area from $D000 is left alone because it is not RAM.
fn place_stubs(load: u16, len: usize) -> Option<Stubs> {
    let lo = usize::from(load);
    let hi = lo + len;
    // $EA31 up to and including the RTI at $EA86.
    let kernal_chain = hi <= usize::from(KERNAL_IRQ) || lo > 0xea86;
    let mut page = 0x0400u16;
    while usize::from(page) + STUB_SPAN <= 0xd000 {
        let p = usize::from(page);
        if p + STUB_SPAN <= lo || p >= hi {
            return Some(Stubs {
                init: page,
                play: page + 8,
                halt: page + 16,
                entry: page + 24,
                ack: page + 32,
                nmi_entry: page + 48,
                nmi_ack: page + 56,
                kernal_chain,
            });
        }
        page += 0x100;
    }
    None
}

const IRQ_VECTOR: u16 = 0xfffe;
const NMI_VECTOR: u16 = 0xfffa;
/// $0318-$0319, NMINV: the RAM vector the KERNAL's NMI entry jumps through.
const NMINV: u16 = 0x0318;
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

/// CIA 1 timer A latch the KERNAL programs for a 1/60 s interrupt, by video
/// standard. Source: the KERNAL ROM's ENABLE TIMER routine at $FDDD, which
/// reads the PAL/NTSC flag in $02A6 and stores `#$25,#$40` (PAL) or
/// `#$95,#$42` (NTSC) to $DC04/$DC05, then jumps to $FF6E, which writes `#$81`
/// to $DC0D (enable the timer A interrupt) and `(CRA & $80) | $11` to $DC0E
/// (force-load and start, continuous mode). Commented listing at
/// <https://www.pagetable.com/c64ref/c64disasm/> (entries $FDDD-$FDF6 and
/// $FF6E-$FF7A). The $02A6 entry of <https://www.pagetable.com/c64ref/c64mem/>
/// describes the same thing: the flag "is used by the routines which set the
/// prescaler values for the system IRQ timer, so that the IRQ occurs every 1/60
/// second". The PSID format (HVSC `SID_file_format.txt`, "The SID file
/// environment") gives the same two values for a CIA-speed song: "CIA 1 timer
/// A: set to 60Hz (0x4025 for PAL and 0x4295 for NTSC) with the counter
/// running. IRQs active when speed flag is 1".
const KERNAL_TIMER_A_PAL: u16 = 0x4025;
const KERNAL_TIMER_A_NTSC: u16 = 0x4295;

/// One VIC-II raster frame, in Φ2 cycles: lines per frame times cycles per
/// line. Source: Christian Bauer, "The MOS 6567/6569 video controller (VIC-II)
/// and its application in the Commodore 64", section 3.4, table of display
/// dimensions: 6569 (PAL-B) 312 lines of 63 cycles; 6567R8 (NTSC-M) 263 lines
/// of 65 cycles; 6567R56A (NTSC-M) 262 lines of 64 cycles. The R8 is chosen
/// for NTSC: the article calls the differences between steppings "mostly
/// negligible with the exception of the 6567R56A", so the R8 is the general
/// 6567 case, and the R56A would need its own option.
/// The PSID format uses the same two products, `312 * 63` and `263 * 65`, in
/// its CIA values for playing a tune on the other video standard.
const PAL_FRAME_CYCLES: u64 = 312 * 63;
const NTSC_FRAME_CYCLES: u64 = 263 * 65;

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

/// The header's fixed size by version. The PSID format (HVSC
/// `SID_file_format.txt`, field `dataOffset` at +06): "Because the header has
/// a fixed size, this value is 0x0076 for version 1 or 0x007C for versions 2
/// through 5." A file needs no more than that before its data block.
const HEADER_LEN_V1: usize = 0x76;
const HEADER_LEN_V2: usize = 0x7c;

/// Why a container was refused. A malformed file is always an error with a named
/// reason, never a guess at a placement and never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PsidError {
    NotPsid,
    /// The file ends inside the header its version declares.
    ShortHeader {
        version: u16,
        need: usize,
        file_len: usize,
    },
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
            Self::NotPsid => write!(f, "not a PSID or RSID file"),
            Self::ShortHeader {
                version,
                need,
                file_len,
            } => write!(
                f,
                "truncated: a v{version} header is {need} bytes but the file is only {file_len} bytes"
            ),
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
    /// The header's three 32-byte text fields.
    name: String,
    author: String,
    released: String,
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
    if bytes.len() < 6 || !matches!(&bytes[0..4], b"PSID" | b"RSID") {
        return Err(PsidError::NotPsid);
    }
    let u16be = |o: usize| u16::from_be_bytes([bytes[o], bytes[o + 1]]);
    let u32be = |o: usize| u32::from_be_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let version = u16be(0x04);
    let need = if version >= 2 {
        HEADER_LEN_V2
    } else {
        HEADER_LEN_V1
    };
    if bytes.len() < need {
        return Err(PsidError::ShortHeader {
            version,
            need,
            file_len: bytes.len(),
        });
    }
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
        name: text_field(&bytes[0x16..0x36]),
        author: text_field(&bytes[0x36..0x56]),
        released: text_field(&bytes[0x56..0x76]),
    })
}

/// A header text field: Latin-1, padded with NULs.
fn text_field(b: &[u8]) -> String {
    b.iter()
        .take_while(|&&c| c != 0)
        .map(|&c| char::from(c))
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The bit of the header's speed field that belongs to a 1-based song.
///
/// The PSID format gives songs 1 to 32 one bit each, from bit 0 up; every song
/// past 32 shares bit 31.
fn speed_bit(song: u16) -> u32 {
    1 << (song.clamp(1, 32) - 1)
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

    let le16 = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
    let mut load = psid.load;
    if load == 0 {
        // The format: the block's first two bytes are the load address, so a
        // block shorter than that holds no tune at all.
        if block.len() < 2 {
            return Err(PsidError::Empty);
        }
        load = le16(block);
        block = &block[2..];
    } else if block.len() > 2 && le16(block) == load.wrapping_add(2) {
        // The block carries its own address prefix even though the header gave an
        // address, so it is placed two bytes further in than the header implies.
        // A block of two bytes or fewer is taken as code: stripping would leave
        // nothing to load.
        block = &block[2..];
    }
    if block.is_empty() {
        return Err(PsidError::Empty);
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
        cia_clock: psid.speed & speed_bit(song) != 0,
        clock_hint: (psid.flags >> 2) & 3,
        chip_hint: (psid.flags >> 4) & 3,
        data: block.to_vec(),
    })
}

/// CIA 1 timer A, modelled to the point where it can raise an interrupt.
///
/// Only what a tune can observe is implemented: the latch, the control
/// register's start bit and force-load strobe, and the interrupt control
/// register with its read-to-clear behaviour. The control register's other
/// bits — port B output, one-shot mode, the CNT and serial sources, the TOD
/// frequency — are stored but have no effect, because this harness has no
/// port B and no clock.
#[derive(Default)]
struct Cia {
    latch: u16,
    counter: u16,
    control: u8,
    /// Set flags; bit 0 is timer A underflow.
    flags: u8,
    /// Which flags are allowed to pull the interrupt line.
    mask: u8,
}

/// CRA bit 0, START, and bit 4, LOAD. MOS 6526 data sheet (Commodore
/// Semiconductor Group), "Control Registers", CRA: "4 LOAD 1=FORCE LOAD (this
/// is a STROBE input, there is no data storage, bit 4 will always read back a
/// zero and writing a zero has no effect)". Under "Interval Timers", "Force
/// Load": "A strobe bit allows the timer latch to be loaded into the timer
/// counter at any time, whether the timer is running or not." Scanned copy at
/// <https://www.zimmers.net/anonftp/pub/cbm/documents/chipdata/6526.zip>.
const CRA_START: u8 = 0x01;
const CRA_LOAD: u8 = 0x10;

impl Cia {
    fn timer_a_running(&self) -> bool {
        self.control & CRA_START != 0
    }

    /// The CIA's output: asserted while any enabled flag is set.
    fn line(&self) -> bool {
        self.flags & self.mask != 0
    }

    /// Timer A as the KERNAL leaves it after reset, which is where every song
    /// finds it before init (see `setup` for why VBL songs too): the 1/60 s
    /// latch for the video standard, the counter loaded from it, running in
    /// continuous mode, and the timer A interrupt enabled. See
    /// [`KERNAL_TIMER_A_PAL`].
    ///
    /// The KERNAL writes `$11` to the control register: start, and the
    /// force-load strobe, which loads the counter. Bit 3 clear is continuous
    /// mode, and this model has no one-shot mode to select.
    fn kernal_timer_a(&mut self, pal: bool) {
        self.latch = if pal {
            KERNAL_TIMER_A_PAL
        } else {
            KERNAL_TIMER_A_NTSC
        };
        self.write_control(CRA_START | CRA_LOAD);
        self.mask |= 0x01;
    }

    /// Advance timer A by one cycle. Returns true on the cycle it underflows,
    /// whether or not the interrupt is enabled or a previous one acknowledged.
    ///
    /// The underflow period is latch+1 cycles: the cycle that underflows also
    /// reloads the counter with the latch, and the counter then needs `latch`
    /// further cycles to reach zero and one more to underflow again. The PSID
    /// format's own CIA values assume the same, subtracting 1 from a period to
    /// get a latch (`312 * 63 * 50 / 60 - 1`). The `cia_timer_a_period_is_latch_plus_one`
    /// test pins it. A latch written while the timer runs is picked up by that
    /// reload, not before.
    fn tick(&mut self) -> bool {
        if !self.timer_a_running() {
            return false;
        }
        if self.counter == 0 {
            self.flags |= 0x01;
            self.counter = self.latch;
            true
        } else {
            self.counter -= 1;
            false
        }
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

    /// A read of one of the 16 registers. Only timer A and the interrupt and
    /// control registers are modelled; the rest read as zero.
    fn read_reg(&mut self, reg: usize) -> u8 {
        match reg {
            4 => (self.counter & 0xff) as u8,
            5 => (self.counter >> 8) as u8,
            0xd => self.read_icr(),
            0xe => self.control,
            _ => 0,
        }
    }

    /// A write to one of the 16 registers; unmodelled ones are ignored.
    fn write_reg(&mut self, reg: usize, value: u8) {
        match reg {
            4 | 5 => {
                if reg == 4 {
                    self.latch = (self.latch & 0xff00) | u16::from(value);
                } else {
                    self.latch = (self.latch & 0x00ff) | (u16::from(value) << 8);
                }
                if !self.timer_a_running() {
                    self.counter = self.latch;
                }
            }
            0xd => self.write_icr(value),
            0xe => self.write_control(value),
            _ => {}
        }
    }

    /// A write to CRA. The LOAD bit is a strobe: it is acted on and never
    /// stored, so it reads back as 0, and with 1 it loads the counter from the
    /// latch whether or not the timer runs (see [`CRA_LOAD`]). Loaded here, the
    /// counter is `latch` on the cycle after the write, so the next underflow
    /// comes latch+1 cycles after the write, the same period as after any
    /// reload (see [`Cia::tick`]).
    fn write_control(&mut self, value: u8) {
        let was_running = self.timer_a_running();
        self.control = value & !CRA_LOAD;
        if value & CRA_LOAD != 0 {
            self.counter = self.latch;
        }
        if self.timer_a_running() {
            if !was_running {
                // Starting the timer loads the counter from the latch.
                self.counter = self.latch;
            }
        } else {
            self.counter = self.latch;
        }
    }
}

/// The VIC-II's raster interrupt and nothing else: a beam position, the compare
/// line, and the interrupt latch and enable registers. Source: Christian Bauer,
/// "The MOS 6567/6569 video controller (VIC-II) and its application in the
/// Commodore 64", section 3.12 (interrupts) and the register list in 3.2:
/// $D011 bit 7 is bit 8 of the raster line ("RST8"), $D012 reads the current
/// line and writes the compare line, $D019 is the latch (bit 0 IRST, bit 7 set
/// while any enabled source is latched, cleared by writing 1 to a bit), and
/// $D01A enables sources. The registers repeat every 64 bytes through
/// $D000-$D3FF. The other sources (sprite collisions, light pen) never fire
/// here, and every other register is plain memory, because nothing else the
/// chip does is audible.
struct Vic {
    line: u16,
    cycle: u16,
    lines: u16,
    cycles_per_line: u16,
    compare: u16,
    /// $D011 as written; bit 7 is replaced by the beam's bit 8 on read.
    control: u8,
    latch: u8,
    enable: u8,
}

/// $D011, $D012, $D019 and $D01A, as offsets in the 64-byte register file.
const VIC_CONTROL: usize = 0x11;
const VIC_RASTER: usize = 0x12;
const VIC_LATCH: usize = 0x19;
const VIC_ENABLE: usize = 0x1a;
/// $D019/$D01A bit 0: the raster source.
const VIC_IRST: u8 = 0x01;

impl Vic {
    /// The line geometry is the one [`PAL_FRAME_CYCLES`] and
    /// [`NTSC_FRAME_CYCLES`] are built from.
    fn new(model: Model) -> Self {
        let (lines, cycles_per_line) = if matches!(model, Model::Pal) {
            (312, 63)
        } else {
            (263, 65)
        };
        Self {
            line: 0,
            cycle: 0,
            lines,
            cycles_per_line,
            compare: 0,
            control: 0,
            latch: 0,
            enable: 0,
        }
    }

    /// Whether an address in $D000-$D3FF is one of the modelled registers.
    fn owns(address: usize) -> bool {
        matches!(
            address & 0x3f,
            VIC_CONTROL | VIC_RASTER | VIC_LATCH | VIC_ENABLE
        )
    }

    /// Advance one cycle. The compare is made as each line starts, so the
    /// latch is set once per frame, on the line that matches.
    fn tick(&mut self) {
        self.cycle += 1;
        if self.cycle == self.cycles_per_line {
            self.cycle = 0;
            self.line += 1;
            if self.line == self.lines {
                self.line = 0;
            }
            if self.line == self.compare {
                self.latch |= VIC_IRST;
            }
        }
    }

    /// The chip's IRQ output: asserted while an enabled source is latched.
    fn line(&self) -> bool {
        self.latch & self.enable & 0x0f != 0
    }

    fn read(&self, reg: usize) -> u8 {
        match reg {
            VIC_CONTROL => (self.control & 0x7f) | (((self.line >> 8) as u8) << 7),
            VIC_RASTER => (self.line & 0xff) as u8,
            // Unused bits read as 1.
            VIC_LATCH => self.latch | 0x70 | if self.line() { 0x80 } else { 0 },
            VIC_ENABLE => self.enable | 0xf0,
            _ => 0,
        }
    }

    fn write(&mut self, reg: usize, value: u8) {
        match reg {
            VIC_CONTROL => {
                self.control = value;
                self.compare = (self.compare & 0xff) | (u16::from(value >> 7) << 8);
            }
            VIC_RASTER => self.compare = (self.compare & 0x100) | u16::from(value),
            // Writing 1 to a latch bit acknowledges it.
            VIC_LATCH => self.latch &= !(value & 0x0f),
            VIC_ENABLE => self.enable = value & 0x0f,
            _ => {}
        }
    }
}

struct Machine {
    ram: [u8; 0x1_0000],
    sid: Sid,
    cia: RefCell<Cia>,
    /// CIA 2, whose line is wired to the processor's NMI input. Tunes use its
    /// timer A to pace sample playback.
    cia2: RefCell<Cia>,
    /// The VIC-II's raster interrupt, for tunes driven by it.
    vic: RefCell<Vic>,
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
            cia2: RefCell::new(Cia::default()),
            vic: RefCell::new(Vic::new(model)),
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
            0xd000..=0xd3ff if Vic::owns(a) => self.vic.borrow().read(a & 0x3f),
            0xdc00..=0xdc0f => self.cia.borrow_mut().read_reg(a - 0xdc00),
            0xdd00..=0xdd0f => self.cia2.borrow_mut().read_reg(a - 0xdd00),
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
            0xd000..=0xd3ff if Vic::owns(a) => self.vic.borrow_mut().write(a & 0x3f, value),
            0xdc00..=0xdc0f => self.cia.borrow_mut().write_reg(a - 0xdc00, value),
            0xdd00..=0xdd0f => self.cia2.borrow_mut().write_reg(a - 0xdd00, value),
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

/// The end of the KERNAL's IRQ handler: `LDA $DC0D / PLA / TAY / PLA / TAX /
/// PLA / RTI`, the bytes at $EA7E-$EA86 in the commented KERNAL listing at
/// <https://www.pagetable.com/c64ref/c64disasm/> (`KPREND LDA D1ICR ;CLEAR
/// INTERUPT FLAGS`, then `;RESTORE REGISTERS`). Reading $DC0D releases CIA 1's
/// line; the pulls undo the entry's pushes in reverse order.
const ACK_AND_RETURN: [u8; 9] = [0xad, 0x0d, 0xdc, 0x68, 0xa8, 0x68, 0xaa, 0x68, 0x40];

/// The IRQ entry the hardware vector leads to: `PHA / TXA / PHA / TYA / PHA /
/// JMP ($0314)`, the KERNAL's entry at $FF48-$FF5A in the same listing
/// (`PULS PHA`, ..., `PULS1 JMP (CINV) ;...IRQ`).
///
/// The KERNAL also looks at the stacked status (`LDA $104,X / AND #$10`) and
/// sends a BRK through $0316 instead. That branch is left out: the KERNAL's
/// BRK handler warm-starts BASIC, which this harness does not have, and a tune
/// that executes BRK has gone wrong either way. Sending it through $0314 keeps
/// what the harness did before for a BRK, which is to acknowledge and return.
fn write_irq_entry(ram: &mut [u8; 0x1_0000], entry: u16) {
    let s = usize::from(entry);
    ram[s..s + 8].copy_from_slice(&[0x48, 0x8a, 0x48, 0x98, 0x48, 0x6c, 0x14, 0x03]);
}

/// The default handler $0314 points at: [`ACK_AND_RETURN`]. It does what the
/// KERNAL's handler does about the interrupt itself and returns to whatever
/// was interrupted. A CIA-speed song's timer runs from before init, so an init
/// that enables interrupts and stays busy past an underflow passes through
/// here and carries on, rather than being stranded with its init unfinished.
fn write_ack_stub(ram: &mut [u8; 0x1_0000], ack: u16) {
    let s = usize::from(ack);
    ram[s..s + ACK_AND_RETURN.len()].copy_from_slice(&ACK_AND_RETURN);
}

/// The NMI entry the hardware vector leads to: `SEI / JMP ($0318)`, the first
/// two instructions of the KERNAL's NMI entry at $FE43 (`NNMI SEI`, `JMP
/// (NMINV)`) in the commented listing at <https://www.pagetable.com/c64ref/c64disasm/>.
const NMI_ENTRY: [u8; 4] = [0x78, 0x6c, 0x18, 0x03];

/// The default handler $0318 points at: `PHA / LDA $DD0D / PLA / RTI`. The
/// KERNAL's own handler (from $FE47) also checks the RESTORE key and the RS-232
/// lines, none of which exist here; what it does about the interrupt is read
/// $DD0D, which clears CIA 2's flags. A tune that enables a CIA 2 interrupt
/// without installing a handler is acknowledged and returned to.
const NMI_ACK: [u8; 6] = [0x48, 0xad, 0x0d, 0xdd, 0x68, 0x40];

/// The KERNAL's own chain entry points, for a $0314 handler that hands over to
/// the KERNAL when it is done, as most do with `JMP $EA31` (or `JMP $EA81` to
/// skip the acknowledge). With no ROM in this flat memory, those addresses
/// would otherwise hold zeros, and the handler would run into a BRK.
///
/// $EA7E-$EA86 get the KERNAL's own bytes ([`ACK_AND_RETURN`]). $EA31 gets
/// `JMP $EA7E`: the KERNAL's handler first updates the jiffy clock, blinks the
/// cursor, checks the cassette and scans the keyboard (from `KEY JSR $FFEA
/// ;UPDATE JIFFY CLOCK` at $EA31), none of which a tune can hear, and then
/// reaches $EA7E. Laid down only when the tune's block does not cover them.
fn write_kernal_chain(ram: &mut [u8; 0x1_0000]) {
    let s = usize::from(KERNAL_IRQ);
    ram[s..s + 3].copy_from_slice(&[
        0x4c,
        (KERNAL_IRQ_END & 0xff) as u8,
        (KERNAL_IRQ_END >> 8) as u8,
    ]);
    let s = usize::from(KERNAL_IRQ_END);
    ram[s..s + ACK_AND_RETURN.len()].copy_from_slice(&ACK_AND_RETURN);
}

/// Lay down the machine state a tune expects to find, before its init runs.
fn write_machine_state(
    ram: &mut [u8; 0x1_0000],
    image: &[u8; 0x400],
    pal: bool,
    load: u16,
    end: u16,
) {
    for (addr, value) in image.iter().enumerate() {
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

/// Where the rendered samples go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SinkKind {
    Wav,
    Audio,
}

/// The one sink type the render loop sees, whichever device is behind it.
type DynSink = Box<dyn Sink<Error = Box<dyn Error>>>;

/// Lets the render loop hold either sink behind one trait object.
struct BoxErr<S>(S);

impl<S> Sink for BoxErr<S>
where
    S: Sink,
    S::Error: Error + 'static,
{
    type Error = Box<dyn Error>;

    fn write(&mut self, samples: &[f32]) -> Result<(), Box<dyn Error>> {
        self.0.write(samples).map_err(Into::into)
    }

    fn finish(&mut self) -> Result<(), Box<dyn Error>> {
        self.0.finish().map_err(Into::into)
    }
}

#[cfg(feature = "playback")]
fn open_audio() -> Result<DynSink, Box<dyn Error>> {
    Ok(Box::new(BoxErr(sid_play::sink::AudioSink::open(
        RATE as u32,
    )?)))
}

#[cfg(not(feature = "playback"))]
fn open_audio() -> Result<DynSink, Box<dyn Error>> {
    Err("this build has no audio output; rebuild with --features playback".into())
}

const USAGE: &str = "usage: render tune.sid [--subsong N] [--seconds S] [--songlengths FILE] [--ntsc] [--probe] [--poweron FILE] [--sink wav|audio] [--out file.wav] [--quiet]";

struct Options {
    tune: Option<PathBuf>,
    /// 0-based, as the register the init routine reads is. Left unset to mean
    /// "whatever the tune's own header asks to start on".
    subsong: Option<u8>,
    /// A fixed length. Unset means "to the end of the tune".
    seconds: Option<f64>,
    /// An HVSC `Songlengths.md5`; looked for above the tune when unset.
    songlengths: Option<PathBuf>,
    /// No progress bar or spectrum, even on a terminal.
    quiet: bool,
    ntsc: bool,
    sink: SinkKind,
    /// Only read by the WAV sink.
    out: PathBuf,
    /// Report the resolved header and stop, without rendering.
    probe: bool,
    /// Record where the tune executes. Inert unless asked for.
    trace: bool,
    /// A 1024-byte file replacing the built-in power-on RAM image.
    poweron: Option<PathBuf>,
}

fn parse_args() -> Result<Options, Box<dyn Error>> {
    let mut o = Options {
        tune: None,
        subsong: None,
        seconds: None,
        songlengths: None,
        quiet: false,
        ntsc: false,
        sink: SinkKind::Wav,
        out: PathBuf::from("out.wav"),
        probe: false,
        trace: false,
        poweron: None,
    };

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--subsong" => {
                let v: u8 = args.next().ok_or("--subsong needs a value")?.parse()?;
                o.subsong = Some(v);
            }
            "--seconds" => {
                o.seconds = Some(args.next().ok_or("--seconds needs a value")?.parse()?);
            }
            "--songlengths" => {
                o.songlengths = Some(PathBuf::from(
                    args.next().ok_or("--songlengths needs a value")?,
                ));
            }
            "--quiet" => o.quiet = true,
            "--sink" => {
                o.sink = match args.next().ok_or("--sink needs a value")?.as_str() {
                    "wav" => SinkKind::Wav,
                    "audio" => SinkKind::Audio,
                    other => return Err(format!("--sink wants wav or audio, got {other}").into()),
                };
            }
            "--out" => o.out = PathBuf::from(args.next().ok_or("--out needs a value")?),
            "--ntsc" => o.ntsc = true,
            "--probe" => o.probe = true,
            "--trace" => o.trace = true,
            "--poweron" => {
                o.poweron = Some(PathBuf::from(args.next().ok_or("--poweron needs a value")?));
            }
            other => o.tune = Some(PathBuf::from(other)),
        }
    }

    Ok(o)
}

/// Place the resolved block in memory and lay down the harness's own stubs.
fn setup(
    model: Model,
    image: &[u8; 0x400],
    loaded: &Loaded,
    pal: bool,
    stubs: Stubs,
) -> Rc<RefCell<Machine>> {
    let machine = Rc::new(RefCell::new(Machine::new(model)));
    {
        let mut m = machine.borrow_mut();
        write_machine_state(
            &mut m.ram,
            image,
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
        // Every song finds CIA 1 timer A as the KERNAL leaves it after reset:
        // running at the 1/60 s latch with its interrupt enabled ($FDDD and
        // $FF6E, see [`KERNAL_TIMER_A_PAL`]). The PSID format's environment
        // has the counter running for every song ("set to 60Hz ... with the
        // counter running"). A CIA-speed song is played on it, and a tune
        // that only rewrites the latch to set its tempo relies on it.
        //
        // The interrupt is enabled for a VBL song too. The format says CIA
        // IRQs are "active when speed flag is 1" and that a VBL song gets a
        // raster IRQ instead, but this harness has no VIC-II to raise one. A
        // VBL song with a play address does not need it: the harness calls
        // play on the raster frame and does not deliver interrupts at all
        // (see `Harness::cycle`). A VBL song with no play address that
        // installs a $0314 handler and enables interrupts does need some
        // periodic interrupt, and on a real machine the KERNAL's timer A is
        // the one running after reset, so it is the stand-in, at 60 Hz rather
        // than the frame rate.
        m.cia.borrow_mut().kernal_timer_a(pal);
        // The KERNAL's interrupt chain: the hardware vector leads to an entry
        // that saves the registers and jumps through $0314, and $0314 leads
        // to a handler that acknowledges CIA 1 and returns. A tune with play
        // address 0 that installs a handler at either level is driven by it;
        // one that installs none is acknowledged and nothing more. A tune
        // with a play address is played by the harness whatever it installs
        // here (see `start`). $0314 is set here even though the power-on image holds
        // the KERNAL's $EA31, because that address is RAM here and may hold
        // the tune.
        write_irq_entry(&mut m.ram, stubs.entry);
        write_ack_stub(&mut m.ram, stubs.ack);
        if stubs.kernal_chain {
            write_kernal_chain(&mut m.ram);
        }
        m.ram[usize::from(CINV)] = (stubs.ack & 0xff) as u8;
        m.ram[usize::from(CINV + 1)] = (stubs.ack >> 8) as u8;
        m.ram[usize::from(IRQ_VECTOR)] = (stubs.entry & 0xff) as u8;
        m.ram[usize::from(IRQ_VECTOR + 1)] = (stubs.entry >> 8) as u8;
        // The same two levels for NMI: $FFFA leads to the KERNAL's entry,
        // which jumps through $0318, and $0318 leads to a handler that
        // acknowledges CIA 2. Set here for the same reason as $0314.
        let e = usize::from(stubs.nmi_entry);
        m.ram[e..e + NMI_ENTRY.len()].copy_from_slice(&NMI_ENTRY);
        let a = usize::from(stubs.nmi_ack);
        m.ram[a..a + NMI_ACK.len()].copy_from_slice(&NMI_ACK);
        m.ram[usize::from(NMINV)] = (stubs.nmi_ack & 0xff) as u8;
        m.ram[usize::from(NMINV + 1)] = (stubs.nmi_ack >> 8) as u8;
        m.ram[usize::from(NMI_VECTOR)] = (stubs.nmi_entry & 0xff) as u8;
        m.ram[usize::from(NMI_VECTOR + 1)] = (stubs.nmi_entry >> 8) as u8;
        m.ram[usize::from(RESET_VECTOR)] = (stubs.halt & 0xff) as u8;
        m.ram[usize::from(RESET_VECTOR + 1)] = (stubs.halt >> 8) as u8;
    }
    machine
}

/// Start a processor on a prepared machine and run its init to completion.
fn start(
    machine: &Rc<RefCell<Machine>>,
    stubs: Stubs,
    pal: bool,
    cia_clock: bool,
    song: u8,
) -> (Cpu, Harness) {
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

    let mut harness = Harness::new(Rc::clone(machine), stubs, pal, cia_clock);
    let mut budget = 20_000_000u64;
    // An IRQ through the default chain mid-init is acknowledged and returned
    // from. One through a vector the tune pointed at the halt loop lands there
    // with S below $FD; only a real return restores it.
    let returned =
        |cpu: &Cpu| cpu.at_instruction_boundary() && cpu.pc == stubs.halt && cpu.s == 0xfd;
    while !returned(&cpu) && !cpu.jammed() && budget > 0 {
        harness.cycle(&mut cpu);
        budget -= 1;
    }
    // The play address decides who drives the tune. The PSID format defines
    // it (+0C playAddress) as "the address of the machine code subroutine
    // called periodically to play the song", and gives 0 as the value that
    // means the tune installs an interrupt handler to call its play routine
    // itself. So a non-zero play address is called by the harness, at the
    // rate the speed bit gives (raster frame or timer A), whatever handler
    // init left at $0314 or $FFFE; a handler there that does not call play,
    // or an init that returns with interrupts masked, must not silence the
    // song. Only a tune with play address 0 is driven by its own interrupts.
    harness.frame_clock = harness.has_play_stub;
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
    /// Cycles into the current raster frame, and a tick that has arrived but
    /// not yet been delivered.
    frame_cycles: u64,
    frame_due: bool,
    /// One raster frame for the video standard; only VBL-speed songs use it.
    frame_period: u64,
    /// The song's speed bit: true means play is driven by CIA 1 timer A
    /// underflows, false by the raster frame.
    cia_clock: bool,
    /// True when the tune has a play address: the harness calls it on the
    /// song's clock and delivers no interrupts. False for a tune with play
    /// address 0, which is driven by its own interrupt handler.
    frame_clock: bool,
    /// Interrupts taken and frames driven, for the summary line.
    irqs: u64,
    frames: u64,
    audio: Vec<f32>,
    /// `--trace` only.
    trace: Trace,
}

impl Harness {
    fn new(machine: Rc<RefCell<Machine>>, stubs: Stubs, pal: bool, cia_clock: bool) -> Self {
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
            frame_period: if pal {
                PAL_FRAME_CYCLES
            } else {
                NTSC_FRAME_CYCLES
            },
            cia_clock,
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
        let underflow = {
            let m = self.machine.borrow();
            m.vic.borrow_mut().tick();
            m.cia2.borrow_mut().tick();
            m.cia.borrow_mut().tick()
        };
        // CIA 2's line is wired to NMI on every tune: nothing else drives that
        // input, and the processor only reacts to its rising edge, so a tune
        // that never enables a CIA 2 interrupt never sees one.
        cpu.set_nmi(self.machine.borrow().cia2.borrow().line());
        if self.frame_clock {
            // The tune has a play address, so the harness is the player. It
            // cannot wait on the processor's interrupt flag, because a tune
            // initialised with interrupts masked never clears it — a real
            // machine reaches the player through the Kernal's own chain, which
            // runs with interrupts enabled by then. Enter the play stub
            // directly, at an instruction boundary, with the processor idle.
            let tick = if self.cia_clock {
                // A CIA-speed song: every timer A underflow is a play call,
                // whatever latch the tune has written since, and none while
                // the tune has stopped the timer through $DC0E.
                underflow
            } else {
                // A VBL-speed song: one call per raster frame, whatever the
                // tune does with CIA 1. The PSID format defines speed bit 0
                // as "vertical blank interrupt timing (50Hz PAL, 60Hz NTSC)",
                // so a timer the tune starts itself does not drive play. The
                // frame keeps its own phase, as the raster does, so a call
                // held back below does not push every later frame back.
                self.frame_cycles += 1;
                if self.frame_cycles >= self.frame_period {
                    self.frame_cycles -= self.frame_period;
                    true
                } else {
                    false
                }
            };
            self.frame_due |= tick;
            // A tick that arrives while an instruction is still running, or
            // while a previous play call has not returned, is held rather than
            // dropped: losing it would silently slow every tune whose play
            // routine happens to straddle the tick.
            if self.frame_due && cpu.at_instruction_boundary() && cpu.pc == self.stubs.halt {
                self.frame_due = false;
                if self.cia_clock {
                    // The handler that calls play on a real machine reads
                    // $DC0D first (the KERNAL's at $EA7E), which clears the
                    // timer A flag. The harness stands in for that handler.
                    self.machine.borrow().cia.borrow_mut().flags &= !0x01;
                }
                cpu.jump(self.stubs.play);
                self.frames += 1;
            }
            // The harness, not the CIA, drives the player here; release any
            // level latched from the last init cycle.
            cpu.set_irq(false);
        } else {
            // The tune has no play address and drives itself, by CIA 1 or the
            // raster interrupt, the same way it is on a real machine: both
            // chips' level-triggered outputs share the processor's IRQ input,
            // which samples it where the hardware does.
            let m = self.machine.borrow();
            cpu.set_irq(m.cia.borrow().line() || m.vic.borrow().line());
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
}

/// The tune's length from an HVSC songlength database, in seconds. A database
/// named on the command line has to be readable; one found by looking above the
/// tune is used only if it can be read.
fn song_length(
    named: Option<&std::path::Path>,
    tune: &std::path::Path,
    bytes: &[u8],
    song: u8,
) -> Result<Option<f64>, Box<dyn Error>> {
    let database = match named {
        Some(p) => {
            std::fs::read(p).map_err(|e| format!("cannot read songlengths {}: {e}", p.display()))?
        }
        None => match songlength::find_database(tune).and_then(|p| std::fs::read(p).ok()) {
            Some(d) => d,
            None => return Ok(None),
        },
    };
    let text = String::from_utf8_lossy(&database);
    Ok(songlength::lookup(&text, &songlength::md5_hex(bytes), song))
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
        return Err(USAGE.into());
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

    let image: [u8; 0x400] = match &opts.poweron {
        Some(p) => std::fs::read(p)?
            .try_into()
            .map_err(|v: Vec<u8>| format!("--poweron wants 1024 bytes, got {}", v.len()))?,
        None => sid_play::poweron::POWERON,
    };
    let machine = setup(model, &image, &loaded, pal, stubs);
    if opts.trace {
        machine.borrow().trace.set(true);
    }
    let (mut cpu, mut harness) = start(&machine, stubs, pal, loaded.cia_clock, loaded.song);

    // How long to play: what the caller asked for, else what the database
    // says, else until the tune falls silent.
    let length = match opts.seconds {
        Some(s) => Length::Known(s),
        None => match song_length(opts.songlengths.as_deref(), &path, &bytes, loaded.song)? {
            Some(s) => Length::Known(s),
            None => Length::UntilSilence(MAX_SECONDS),
        },
    };
    let total = match length {
        Length::Known(s) | Length::UntilSilence(s) => (s * RATE) as usize,
    };
    let silence_after = (SILENCE_SECONDS * RATE) as usize;
    let never_heard_after = (NEVER_HEARD_SECONDS * RATE) as usize;

    let mut screen = (!opts.quiet && std::io::stderr().is_terminal()).then(|| {
        let mut title = match (psid.name.as_str(), psid.author.as_str()) {
            ("", "") => path.display().to_string(),
            (name, "") => name.to_owned(),
            (name, author) => format!("{name} \u{b7} {author}"),
        };
        if !psid.released.is_empty() {
            title.push_str(&format!(" ({})", psid.released));
        }
        title.push_str(&format!(
            "  \x1b[0;2msong {}/{}, {}\x1b[0m",
            loaded.song,
            loaded.songs,
            if pal { "PAL" } else { "NTSC" }
        ));
        // Live audio reaches the speaker one ring buffer after it is rendered.
        let delay = match opts.sink {
            SinkKind::Audio => (sid_play::sink::LIVE_BUFFER_SECONDS * RATE) as usize,
            SinkKind::Wav => 0,
        };
        (Screen::new(title, RATE as f32, delay), delay)
    });
    let mut sink: DynSink = match opts.sink {
        SinkKind::Wav => Box::new(BoxErr(WavSink::create(
            &opts.out,
            WavSpec {
                sample_rate: RATE as u32,
                channels: 1,
                format: SampleFormat::Pcm16,
            },
        )?)),
        SinkKind::Audio => open_audio()?,
    };

    let mut written = 0usize;
    let mut cycles = 0u64;
    // Only kept under `--trace`, to report what the render actually sounds like
    // alongside where it went.
    let mut heard: Vec<f32> = Vec::new();
    // Samples since the last one louder than `SILENCE_LEVEL`, and whether any
    // has been.
    let mut quiet_run = 0usize;
    let mut sounded = false;
    while written < total {
        harness.cycle(&mut cpu);
        cycles += 1;
        if harness.audio.len() < 4096 {
            continue;
        }
        // The last block is cut to the length asked for, so a known length is
        // rendered exactly.
        let block = &harness.audio[..harness.audio.len().min(total - written)];
        if opts.trace {
            heard.extend_from_slice(block);
        }
        sink.write(block)?;
        written += block.len();
        for &s in block {
            if s.abs() > SILENCE_LEVEL {
                quiet_run = 0;
                sounded = true;
            } else {
                quiet_run += 1;
            }
        }
        if let Some((screen, delay)) = screen.as_mut() {
            screen.feed(block);
            screen.draw(written.saturating_sub(*delay) as f64 / RATE, length, false)?;
        }
        harness.audio.clear();
        if matches!(length, Length::UntilSilence(_))
            && ((sounded && quiet_run >= silence_after)
                || (!sounded && written >= never_heard_after))
        {
            break;
        }
    }
    sink.finish()?;
    if let Some((screen, _)) = screen.as_mut() {
        screen.draw(written as f64 / RATE, length, true)?;
    }

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
    let dest = match opts.sink {
        SinkKind::Wav => format!(
            "wrote {} ({} bytes, ",
            opts.out.display(),
            std::fs::metadata(&opts.out)?.len()
        ),
        SinkKind::Audio => "played (".to_owned(),
    };
    println!(
        "{}{} samples, load ${:04X}, init ${:04X}, play ${:04X}, song {}/{}, {} cycles, {} frames driven, {} IRQs taken, {} chip writes, final $d418=${:02X} $d404=${:02X} freq={})",
        dest,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Cycles from one underflow to the next, after the first.
    fn period(cia: &mut Cia) -> u64 {
        while !cia.tick() {}
        let mut n = 1;
        while !cia.tick() {
            n += 1;
        }
        n
    }

    #[test]
    fn cia_timer_a_period_is_latch_plus_one() {
        let mut cia = Cia::default();
        cia.kernal_timer_a(true);
        assert_eq!(period(&mut cia), u64::from(KERNAL_TIMER_A_PAL) + 1);
        let mut cia = Cia::default();
        cia.kernal_timer_a(false);
        assert_eq!(period(&mut cia), u64::from(KERNAL_TIMER_A_NTSC) + 1);
    }

    #[test]
    fn the_kernal_state_raises_the_interrupt_line() {
        let mut cia = Cia::default();
        cia.kernal_timer_a(true);
        assert!(cia.timer_a_running());
        assert_eq!(cia.counter, KERNAL_TIMER_A_PAL);
        while !cia.tick() {}
        assert!(cia.line(), "timer A interrupt is not enabled");
    }

    /// CRA bit 4 is a force-load strobe: writing it with 1 loads the counter
    /// from the latch at once, even while the timer runs, and it reads back as
    /// 0 (6526 data sheet, CRA bit 4 "LOAD").
    #[test]
    fn force_load_reloads_a_running_timer_and_reads_back_clear() {
        let mut m = Machine::new(Model::Pal);
        m.cia.borrow_mut().kernal_timer_a(true);
        for _ in 0..100 {
            m.cia.borrow_mut().tick();
        }
        m.write(0xdc04, 0x00);
        m.write(0xdc05, 0x01);
        m.write(0xdc0e, 0x11);
        assert_eq!(m.read(0xdc0e), 0x01, "the force-load strobe was stored");
        let mut n = 1u64;
        while !m.cia.borrow_mut().tick() {
            n += 1;
        }
        assert_eq!(n, 0x0100 + 1, "first underflow after a force load of $0100");
    }

    /// The KERNAL chain stand-ins ($EA31 and $EA7E-$EA86) are laid down only
    /// when the tune's block leaves all of $EA31-$EA86 free. The block is the
    /// half-open range [load, load + len).
    #[test]
    fn the_kernal_chain_is_written_only_when_the_block_leaves_it_free() {
        let chain = |load: u16, len: usize| place_stubs(load, len).unwrap().kernal_chain;
        // Ends exactly at $EA31, so $EA31 itself is free.
        assert!(chain(0xe000, 0xa31));
        // Ends one byte later, covering $EA31.
        assert!(!chain(0xe000, 0xa32));
        // Starts on the RTI at $EA86.
        assert!(!chain(0xea86, 0x10));
        // Starts just past it.
        assert!(chain(0xea87, 0x10));
        // Covers $EA7E-$EA86 only.
        assert!(!chain(0xea80, 0x10));
        // Nowhere near.
        assert!(chain(0x1000, 0x100));
    }

    /// When the tune's block covers the chain, its own bytes stay there: the
    /// harness writes neither $EA31 nor $EA7E-$EA86, even the part of that
    /// range the block does not reach.
    #[test]
    fn a_block_over_the_kernal_chain_keeps_its_own_bytes() {
        // $E000-$EA4F, filled with $EE: covers $EA31 but not $EA7E.
        let loaded = Loaded {
            load: 0xe000,
            init: 0xe000,
            play: 0,
            song: 1,
            songs: 1,
            cia_clock: true,
            clock_hint: 0,
            chip_hint: 0,
            data: vec![0xee; 0xa50],
        };
        let stubs = place_stubs(loaded.load, loaded.data.len()).unwrap();
        assert!(!stubs.kernal_chain);
        let m = setup(
            Model::Pal,
            &sid_play::poweron::POWERON,
            &loaded,
            true,
            stubs,
        );
        let m = m.borrow();
        assert_eq!(&m.ram[0xea31..0xea34], &[0xee; 3], "$EA31 was overwritten");
        assert_eq!(
            &m.ram[0xea7e..0xea87],
            &[0; 9],
            "$EA7E-$EA86 was written although the chain was not laid down"
        );
        // The default handler still goes to the harness's own stub.
        let cinv = u16::from_le_bytes([m.ram[0x0314], m.ram[0x0315]]);
        assert_eq!(cinv, stubs.ack);
    }

    /// CIA 1 timer A is in the KERNAL's post-reset state for every song, not
    /// only for CIA-speed ones: running at the 1/60 s latch with its
    /// interrupt enabled.
    #[test]
    fn every_song_finds_the_kernal_timer_running() {
        for cia_clock in [false, true] {
            let loaded = Loaded {
                load: 0x1000,
                init: 0x1000,
                play: 0x1000,
                song: 1,
                songs: 1,
                cia_clock,
                clock_hint: 0,
                chip_hint: 0,
                data: vec![0x60],
            };
            let stubs = place_stubs(loaded.load, loaded.data.len()).unwrap();
            let m = setup(
                Model::Pal,
                &sid_play::poweron::POWERON,
                &loaded,
                true,
                stubs,
            );
            let m = m.borrow();
            let cia = m.cia.borrow();
            assert!(
                cia.timer_a_running(),
                "cia_clock={cia_clock}: timer stopped"
            );
            assert_eq!(cia.latch, KERNAL_TIMER_A_PAL, "cia_clock={cia_clock}");
            assert_eq!(cia.mask & 0x01, 0x01, "cia_clock={cia_clock}: IRQ off");
        }
    }

    /// Songs past 32 take their speed from bit 31, as the PSID format says,
    /// instead of shifting past the end of the field.
    #[test]
    fn songs_past_32_use_speed_bit_31() {
        assert_eq!(speed_bit(1), 1);
        assert_eq!(speed_bit(32), 1 << 31);
        assert_eq!(speed_bit(33), 1 << 31);
        assert_eq!(speed_bit(255), 1 << 31);

        let bytes = [0x00, 0x10, 0x60]; // load prefix $1000, then RTS
        let psid = |speed: u32| Psid {
            version: 2,
            data_off: 0,
            load: 0,
            init: 0,
            play: 0x1000,
            songs: 64,
            start_song: 1,
            speed,
            flags: 0,
            name: String::new(),
            author: String::new(),
            released: String::new(),
        };
        for (wanted, speed, cia) in [
            (31u8, 1u32 << 31, true),   // song 32
            (32, 1 << 31, true),        // song 33
            (63, 1 << 31, true),        // song 64
            (40, !(1u32 << 31), false), // song 41: every other bit set
            (30, 1 << 31, false),       // song 31 has its own bit
        ] {
            let loaded = resolve(&psid(speed), &bytes, Some(wanted)).unwrap();
            assert_eq!(loaded.cia_clock, cia, "song {}", loaded.song);
        }
    }

    /// RSID files share the PSID header layout; only the magic differs.
    #[test]
    fn rsid_magic_is_accepted() {
        let mut bytes = vec![0u8; HEADER_LEN_V2];
        bytes[5] = 2;
        for (magic, ok) in [(b"PSID", true), (b"RSID", true), (b"XSID", false)] {
            bytes[..4].copy_from_slice(magic);
            assert_eq!(
                parse(&bytes).is_ok(),
                ok,
                "{}",
                String::from_utf8_lossy(magic)
            );
        }
    }

    /// The raster latch is set once per frame, on the compare line, and an
    /// acknowledge through $D019 releases the line.
    #[test]
    fn the_raster_interrupt_fires_once_per_frame() {
        let mut vic = Vic::new(Model::Pal);
        vic.write(VIC_RASTER, 1);
        vic.write(VIC_ENABLE, VIC_IRST);
        let mut fired = 0;
        for _ in 0..3 * PAL_FRAME_CYCLES {
            vic.tick();
            if vic.line() {
                fired += 1;
                assert_eq!(vic.read(VIC_RASTER), 1);
                assert_eq!(vic.read(VIC_LATCH) & 0x81, 0x81);
                vic.write(VIC_LATCH, VIC_IRST);
                assert!(!vic.line());
            }
        }
        assert_eq!(fired, 3);
    }

    /// $D011 bit 7 is bit 8 of the compare line, and reads back as bit 8 of
    /// the beam's.
    #[test]
    fn d011_bit_7_extends_the_raster_line() {
        let mut vic = Vic::new(Model::Pal);
        vic.write(VIC_CONTROL, 0x80);
        vic.write(VIC_RASTER, 0x05);
        vic.write(VIC_ENABLE, VIC_IRST);
        while !vic.line() {
            vic.tick();
        }
        assert_eq!(vic.line, 0x105);
        assert_eq!(vic.read(VIC_CONTROL) & 0x80, 0x80);
        assert_eq!(vic.read(VIC_RASTER), 0x05);
    }

    /// A tune with no play address, as every RSID is, that silences CIA 1,
    /// drives itself from a raster IRQ and paces samples with CIA 2 NMIs. Its
    /// handlers count calls at $2000 (IRQ) and $2001 (NMI).
    #[test]
    fn raster_irq_and_cia2_nmi_drive_a_tune() {
        #[rustfmt::skip]
        let code = vec![
            0x78,                               // $1000 SEI
            0xa9, 0x7f, 0x8d, 0x0d, 0xdc,       // CIA 1 interrupts off
            0xa9, 0x3a, 0x8d, 0x14, 0x03,       // $0314 = irq ($103A)
            0xa9, 0x10, 0x8d, 0x15, 0x03,
            0xa9, 0x45, 0x8d, 0x18, 0x03,       // $0318 = nmi ($1045)
            0xa9, 0x10, 0x8d, 0x19, 0x03,
            0xa9, 0x00, 0x8d, 0x12, 0xd0,       // raster compare line 0
            0xa9, 0x01, 0x8d, 0x1a, 0xd0,       // raster IRQ on
            0xa9, 0xff, 0x8d, 0x04, 0xdd,       // CIA 2 latch $03FF
            0xa9, 0x03, 0x8d, 0x05, 0xdd,
            0xa9, 0x81, 0x8d, 0x0d, 0xdd,       // CIA 2 timer A NMI on
            0xa9, 0x11, 0x8d, 0x0e, 0xdd,       // start, force load
            0x58, 0x60,                         // CLI, RTS
            // $103A irq: INC $2000, ack the raster, KERNAL register restore
            0xee, 0x00, 0x20, 0xa9, 0x01, 0x8d, 0x19, 0xd0, 0x4c, 0x81, 0xea,
            // $1045 nmi: PHA, LDA $DD0D, INC $2001, PLA, RTI
            0x48, 0xad, 0x0d, 0xdd, 0xee, 0x01, 0x20, 0x68, 0x40,
        ];
        let loaded = Loaded {
            load: 0x1000,
            init: 0x1000,
            play: 0,
            song: 1,
            songs: 1,
            cia_clock: false,
            clock_hint: 0,
            chip_hint: 0,
            data: code,
        };
        let stubs = place_stubs(loaded.load, loaded.data.len()).unwrap();
        let machine = setup(
            Model::Pal,
            &sid_play::poweron::POWERON,
            &loaded,
            true,
            stubs,
        );
        let (mut cpu, mut harness) = start(&machine, stubs, true, false, 1);
        assert!(harness.trace.init_returned, "init did not return");
        for _ in 0..3 * PAL_FRAME_CYCLES {
            harness.cycle(&mut cpu);
        }
        let m = machine.borrow();
        let (irqs, nmis) = (m.ram[0x2000], m.ram[0x2001]);
        assert!((2..=3).contains(&irqs), "raster IRQs: {irqs}");
        // One NMI per 1024 cycles.
        let expect = 3 * PAL_FRAME_CYCLES / 1024;
        assert!(
            u64::from(nmis).abs_diff(expect) <= 2,
            "NMIs: {nmis}, expected about {expect}"
        );
    }

    #[test]
    fn the_frame_constants_are_the_vic_ii_raster_frames() {
        assert_eq!(PAL_FRAME_CYCLES, 19_656);
        assert_eq!(NTSC_FRAME_CYCLES, 17_095);
    }
}
