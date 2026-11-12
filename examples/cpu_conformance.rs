//! Runs Klaus Dormann's 6502 functional test against `lib6502`.
//!
//! The test image is a flat 64 KiB binary entered at `$0400`. Every failure is a
//! `JMP *` trap, and so is success, so the run ends when the program counter
//! stops moving; the address it stopped at names the failed test in the
//! listing (`6502_functional_test.lst`).
//!
//! ```sh
//! cargo run --release --example cpu_conformance -- 6502_functional_test.bin 3469
//! ```
//!
//! The second argument is the success trap address in hex, taken from the
//! listing of the build being run. With `PATCH` set in the environment, the
//! instructions in `render/cpu_fixups.rs` are executed by hand rather than by
//! `lib6502`, the same way the renderer runs them, so the trap reached is the
//! first defect not yet worked around.

use lib6502::Cpu;
use lib6502::bus::Bus;

struct Flat(Box<[u8; 0x1_0000]>);

impl Bus for Flat {
    fn read(&self, address: u16) -> u8 {
        self.0[usize::from(address)]
    }
    fn write(&mut self, address: u16, value: u8) {
        self.0[usize::from(address)] = value;
    }
    fn load(&mut self, _path: &str, _address: u16) {}
}

#[path = "render/cpu_fixups.rs"]
mod cpu_fixups;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(path), Some(ok)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: cpu_conformance <image.bin> <success-trap-hex>");
        return std::process::ExitCode::FAILURE;
    };
    let ok = match u16::from_str_radix(ok.trim_start_matches('$'), 16) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("bad trap address: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let image = match std::fs::read(path) {
        Ok(b) if b.len() == 0x1_0000 => b,
        Ok(b) => {
            eprintln!("image is {} bytes, expected 65536", b.len());
            return std::process::ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut mem = Box::new([0u8; 0x1_0000]);
    mem.copy_from_slice(&image);
    let mut cpu = Cpu::new(Box::new(Flat(mem)), 0);
    cpu.pc = 0x0400;

    let mut instructions: u64 = 0;
    let mut last = cpu.pc;
    let patch = std::env::var("PATCH").is_ok();
    loop {
        // One whole instruction: the first clock executes it, the rest burn
        // its remaining cycles.
        if patch {
            cpu_fixups::step(&mut cpu);
        }
        cpu.clock();
        while cpu.cycles > 0 {
            cpu.clock();
        }
        instructions += 1;
        if cpu.pc == last {
            break;
        }
        last = cpu.pc;
        if instructions > 200_000_000 {
            println!("RESULT timeout pc=${:04X}", cpu.pc);
            return std::process::ExitCode::FAILURE;
        }
    }
    let passed = cpu.pc == ok;
    println!(
        "RESULT {} trap=${:04X} instructions={} a=${:02X} x=${:02X} y=${:02X} p=${:02X} sp=${:02X}",
        if passed { "pass" } else { "FAIL" },
        cpu.pc,
        instructions,
        cpu.a,
        cpu.x,
        cpu.y,
        cpu.status,
        cpu.sp
    );
    if passed {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
