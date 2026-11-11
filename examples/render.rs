//! PSID harness: runs a tune's 6510 code and feeds the writes to the chip.
//!
//! The tune's own code owns everything it was loaded over, so the harness keeps
//! its footprint to the top of the last page: two call stubs and a halt address,
//! all above $ffe0. Nothing else is written outside the load block.
//!
//! `cargo run --release --example render -- tune.sid [--subsong N] [--seconds S]
//!         [--ntsc] [--out file.wav]`

use mos6502::Variant;
use mos6502::cpu::CPU;
use mos6502::instruction::Nmos6502;
use mos6502::memory::Bus;
use mos6502::registers::{Registers, StackPointer, Status};
use rust_sid::sink::{SampleFormat, Sink, WavSink, WavSpec};
use rust_sid::{Model, Sid};
use std::path::PathBuf;

const RATE: f64 = 44_100.0;

/// `JSR <entry>` followed by `JMP HALT`, so the tune's `RTS` brings us back to
/// the stub and the stub lands us on the halt address. Using the processor's own
/// `JSR` means the stack frame is laid out exactly as the tune expects it.
const STUB_INIT: u16 = 0xffe0;
const STUB_PLAY: u16 = 0xffe8;
/// Where the CPU parks once a routine has returned.
const HALT: u16 = 0xfff0;
/// `RTI`, for a tune that takes the interrupt path instead of being called.
const IRQ_STUB: u16 = 0xfff4;

struct Machine {
    ram: [u8; 0x1_0000],
    sid: Sid,
    /// CIA timer 1 latch, low then high byte. Tunes that want a frame rate other
    /// than 60 Hz program this.
    timer: [u8; 2],
}

impl Bus for Machine {
    fn get_byte(&mut self, address: u16) -> u8 {
        let a = usize::from(address);
        match a {
            0xd400..=0xd5ff => self.sid.read(a - 0xd400),
            0xdc00..=0xdc0f => match a - 0xdc00 {
                4 => self.timer[0],
                5 => self.timer[1],
                _ => 0,
            },
            _ => self.ram[a],
        }
    }

    fn set_byte(&mut self, address: u16, value: u8) {
        let a = usize::from(address);
        match a {
            0xd400..=0xd5ff => self.sid.write(a - 0xd400, value),
            0xdc00..=0xdc0f => match a - 0xdc00 {
                4 => self.timer[0] = value,
                5 => self.timer[1] = value,
                _ => {}
            },
            _ => self.ram[a] = value,
        }
    }
}

/// Cycles one frame should last. A tune that programs the CIA timer gets its
/// rate; one that never touches it is played at 60 Hz.
fn frame_cycles(machine: &Machine, clock_hz: f64) -> u64 {
    let latched = u16::from_le_bytes(machine.timer);
    if latched == 0 {
        (clock_hz / 60.0) as u64
    } else {
        u64::from(latched)
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

/// Enter the routine at `stub` and run until it returns to the halt address.
/// Only the program counter is set: registers, stack pointer and memory all keep
/// whatever the previous call left behind, which is what a real interrupt entry
/// does and what a tune's sequencing variables depend on.
///
/// Returns the Φ2 cycles the call consumed.
fn call_routine<M: Bus, V: Variant>(cpu: &mut CPU<M, V>, stub: u16, budget: u64) -> u64 {
    cpu.registers.program_counter = stub;
    let start = cpu.cycles;
    let mut steps = 0u64;
    while cpu.registers.program_counter != HALT && steps < budget {
        cpu.single_step();
        steps += 1;
    }
    cpu.cycles.wrapping_sub(start)
}

#[derive(Default)]
struct Options {
    tune: Option<PathBuf>,
    subsong: u8,
    seconds: f64,
    ntsc: bool,
    out: PathBuf,
}

fn parse_args() -> Result<Options, Box<dyn std::error::Error>> {
    let mut o = Options {
        seconds: 30.0,
        out: PathBuf::from("out.wav"),
        ..Options::default()
    };

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--subsong" => o.subsong = args.next().ok_or("--subsong needs a value")?.parse()?,
            "--seconds" => o.seconds = args.next().ok_or("--seconds needs a value")?.parse()?,
            "--out" => o.out = PathBuf::from(args.next().ok_or("--out needs a value")?),
            "--ntsc" => o.ntsc = true,
            other => o.tune = Some(PathBuf::from(other)),
        }
    }

    Ok(o)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = parse_args()?;
    let Some(path) = opts.tune else {
        return Err(
            "usage: render tune.sid [--subsong N] [--seconds S] [--ntsc] [--out file.wav]".into(),
        );
    };
    let subsong = opts.subsong;
    let seconds = opts.seconds;
    let ntsc = opts.ntsc;
    let out = opts.out;

    let model = if ntsc { Model::Ntsc } else { Model::Pal };
    let clock_hz = model.clock_hz();

    let bytes = std::fs::read(&path)?;
    if bytes.len() < 128 || &bytes[0..4] != b"PSID" {
        return Err("not a PSID file".into());
    }
    let header = &bytes[..128];
    let u16be = |o: usize| u16::from_be_bytes([header[o], header[o + 1]]);
    // The data offset is measured from the start of the C64-side header, which
    // begins after the four-byte file magic. A zero data length means the block
    // runs to the end of the file.
    let data_off = 4 + usize::from(u16be(0x06));
    let data_len = match usize::from(u16be(0x08)) {
        0 => bytes.len().saturating_sub(data_off),
        n => n,
    };
    let init = u16be(0x0a);
    let play = u16be(0x0c);
    let load_hdr = u16be(0x0e);

    let mut machine = Machine {
        ram: [0; 0x1_0000],
        sid: Sid::new(model, RATE),
        timer: [0, 0],
    };

    let load = usize::from(load_hdr);
    if load == 0 {
        return Err("relocating second-part tunes are not supported".into());
    }
    let start = load;
    let end = start.saturating_add(data_len).min(0xd000);
    let available = bytes.len().saturating_sub(data_off).min(end - start);
    if available == 0 {
        return Err("the tune's data block is empty".into());
    }
    machine.ram[start..start + available].copy_from_slice(&bytes[data_off..data_off + available]);

    write_call_stub(&mut machine.ram, STUB_INIT, init);
    write_call_stub(&mut machine.ram, STUB_PLAY, play);
    machine.ram[usize::from(IRQ_STUB)] = 0x40; // RTI
    machine.ram[0xfffe] = (IRQ_STUB & 0xff) as u8;
    machine.ram[0xffff] = (IRQ_STUB >> 8) as u8;

    let mut cpu: CPU<Machine, Nmos6502> = CPU::new(machine, Nmos6502);

    // The processor's own reset sequence, then the register state a Kernal
    // warm-start would leave: interrupts masked, stack at the top of the page.
    cpu.registers = Registers::new();
    cpu.registers.stack_pointer = StackPointer(0xfd);
    cpu.registers.status = Status::PS_DISABLE_INTERRUPTS | Status::PS_UNUSED;
    // The PSID convention passes the starting subsong to the init routine in A.
    cpu.registers.accumulator = subsong;

    cpu.memory.sid.write(24, 0x0f); // full volume; the tune sets the taps itself
    call_routine(&mut cpu, STUB_INIT, 2_000_000);

    let total = (seconds * RATE) as usize;
    let mut sink = WavSink::create(
        &out,
        WavSpec {
            sample_rate: RATE as u32,
            channels: 1,
            format: SampleFormat::Pcm16,
        },
    )?;

    let mut written = 0usize;
    let mut audio: Vec<f32> = Vec::new();
    while written < total {
        let used = call_routine(&mut cpu, STUB_PLAY, 200_000);
        // The play call's cycles are part of the frame, not extra to it, so the
        // frame is never shorter than the work the tune just did.
        let frame = frame_cycles(&cpu.memory, clock_hz).max(used);

        audio.clear();
        cpu.memory.sid.render_cycles(frame, &mut audio);
        if audio.is_empty() {
            continue;
        }
        sink.write(&audio)?;
        written += audio.len();
    }
    sink.finish()?;

    let bytes_out = std::fs::metadata(&out)?.len();
    println!(
        "wrote {} ({} bytes, {} frames)",
        out.display(),
        bytes_out,
        sink.samples_written()
    );
    Ok(())
}
