//! Instructions `lib6502` 0.2.2 executes incorrectly, executed here instead.
//!
//! Found by running Klaus Dormann's 6502 functional test (see
//! `examples/cpu_conformance.rs`). Each entry is a defect in the crate:
//!
//! - `ADC`/`SBC` on the NMOS and CMOS variants never update C, N or V in binary
//!   mode (only the NES branch sets them), `ADC` never clears Z, and `SBC`
//!   subtracts the carry instead of the borrow.
//! - `PHP` ORs `get_flag(..) as u8` (0 or 1) into the pushed byte, so it sets
//!   Carry instead of B and bit 5, and then clears B and bit 5 in P.
//! - `BRK` pushes P without bit 5.
//! - `(zp),Y` never adds Y, and `(zp,X)` does not wrap inside the zero page.
//!   Every group-one instruction (`ORA AND EOR ADC STA LDA CMP SBC`) is
//!   therefore executed here in all its addressing modes.
//!
//! [`step`] is called at an instruction boundary. When it handles the opcode
//! it performs the whole instruction and loads `cpu.cycles` with its full
//! length, so the caller's next `cpu.clock()` only counts cycles down.

use lib6502::Cpu;

const C: u8 = 0x01;
const Z: u8 = 0x02;
const I: u8 = 0x04;
const D: u8 = 0x08;
const V: u8 = 0x40;
const N: u8 = 0x80;

fn set(p: &mut u8, flag: u8, on: bool) {
    if on {
        *p |= flag;
    } else {
        *p &= !flag;
    }
}

fn read16_zp(cpu: &Cpu, zp: u8) -> u16 {
    u16::from_le_bytes([
        cpu.read(u16::from(zp)),
        cpu.read(u16::from(zp.wrapping_add(1))),
    ])
}

fn page_cross(base: u16, addr: u16) -> u8 {
    u8::from(base & 0xff00 != addr & 0xff00)
}

/// Where a group-one opcode (`ORA AND EOR ADC STA LDA CMP SBC`, low bits
/// `01`, plus the undocumented `SBC #` at `$EB`) finds its operand: the
/// address (`None` for immediate), the instruction length and its cycle count.
fn group_one(cpu: &Cpu, op: u8) -> (Option<u16>, u16, u8) {
    let pc = cpu.pc;
    let b1 = cpu.read(pc.wrapping_add(1));
    let w = u16::from_le_bytes([b1, cpu.read(pc.wrapping_add(2))]);
    // Stores always take the page-cross cycle; loads only when they cross.
    let store = op & 0xe0 == 0x80;
    let indexed = |base: u16, a: u16, fixed: u8| fixed + u8::from(store).max(page_cross(base, a));
    if op == 0xeb {
        return (None, 2, 2);
    }
    match op & 0x1c {
        0x08 => (None, 2, 2),
        0x04 => (Some(u16::from(b1)), 2, 3),
        0x14 => (Some(u16::from(b1.wrapping_add(cpu.x))), 2, 4),
        0x0c => (Some(w), 3, 4),
        0x1c => {
            let a = w.wrapping_add(u16::from(cpu.x));
            (Some(a), 3, indexed(w, a, 4))
        }
        0x18 => {
            let a = w.wrapping_add(u16::from(cpu.y));
            (Some(a), 3, indexed(w, a, 4))
        }
        0x00 => (Some(read16_zp(cpu, b1.wrapping_add(cpu.x))), 2, 6),
        _ => {
            // 0x10: (zp),Y
            let base = read16_zp(cpu, b1);
            let a = base.wrapping_add(u16::from(cpu.y));
            (Some(a), 2, indexed(base, a, 5))
        }
    }
}

fn adc(cpu: &mut Cpu, m: u8) {
    let a = cpu.a;
    let c = u16::from(cpu.status & C);
    let bin = u16::from(a) + u16::from(m) + c;
    let mut p = cpu.status;
    set(&mut p, Z, bin & 0xff == 0);
    if p & D == 0 {
        set(&mut p, N, bin & 0x80 != 0);
        set(&mut p, V, (!(a ^ m) & (a ^ bin as u8)) & 0x80 != 0);
        set(&mut p, C, bin > 0xff);
        cpu.a = bin as u8;
    } else {
        // NMOS decimal mode: N and V come from the half-adjusted result.
        let mut lo = u16::from(a & 0x0f) + u16::from(m & 0x0f) + c;
        if lo > 0x09 {
            lo = ((lo + 0x06) & 0x0f) + 0x10;
        }
        let mut r = u16::from(a & 0xf0) + u16::from(m & 0xf0) + lo;
        set(&mut p, N, r & 0x80 != 0);
        set(&mut p, V, (!(a ^ m) & (a ^ r as u8)) & 0x80 != 0);
        if r > 0x9f {
            r += 0x60;
        }
        set(&mut p, C, r > 0xff);
        cpu.a = r as u8;
    }
    cpu.status = p;
}

fn sbc(cpu: &mut Cpu, m: u8) {
    let a = cpu.a;
    let borrow = i16::from(1 - (cpu.status & C));
    let bin = i16::from(a) - i16::from(m) - borrow;
    let mut p = cpu.status;
    // NMOS: every flag comes from the binary result, in both modes.
    set(&mut p, Z, bin & 0xff == 0);
    set(&mut p, N, bin & 0x80 != 0);
    set(&mut p, V, ((a ^ m) & (a ^ bin as u8)) & 0x80 != 0);
    set(&mut p, C, bin >= 0);
    if p & D == 0 {
        cpu.a = bin as u8;
    } else {
        let mut lo = i16::from(a & 0x0f) - i16::from(m & 0x0f) - borrow;
        if lo < 0 {
            lo = ((lo - 0x06) & 0x0f) - 0x10;
        }
        let mut r = i16::from(a & 0xf0) - i16::from(m & 0xf0) + lo;
        if r < 0 {
            r -= 0x60;
        }
        cpu.a = r as u8;
    }
    cpu.status = p;
}

fn push(cpu: &mut Cpu, v: u8) {
    cpu.write(0x0100 | u16::from(cpu.sp), v);
    cpu.sp = cpu.sp.wrapping_sub(1);
}

/// Execute the instruction at `cpu.pc` if it is one `lib6502` gets wrong.
/// Returns whether it was handled. Must only be called with `cpu.cycles == 0`.
pub fn step(cpu: &mut Cpu) -> bool {
    let op = cpu.read(cpu.pc);
    match op {
        _ if op & 0x03 == 0x01 || op == 0xeb => {
            if op == 0x89 {
                return false; // NOP #, not STA #
            }
            let (addr, len, cycles) = group_one(cpu, op);
            let fetch = |cpu: &Cpu| match addr {
                Some(a) => cpu.read(a),
                None => cpu.read(cpu.pc.wrapping_add(1)),
            };
            let zn = |cpu: &mut Cpu, v: u8| {
                set(&mut cpu.status, Z, v == 0);
                set(&mut cpu.status, N, v & 0x80 != 0);
            };
            match op >> 5 {
                0 => {
                    cpu.a |= fetch(cpu);
                    zn(cpu, cpu.a);
                }
                1 => {
                    cpu.a &= fetch(cpu);
                    zn(cpu, cpu.a);
                }
                2 => {
                    cpu.a ^= fetch(cpu);
                    zn(cpu, cpu.a);
                }
                3 => {
                    let m = fetch(cpu);
                    adc(cpu, m);
                }
                4 => {
                    if let Some(a) = addr {
                        cpu.write(a, cpu.a);
                    }
                }
                5 => {
                    cpu.a = fetch(cpu);
                    zn(cpu, cpu.a);
                }
                6 => {
                    let m = fetch(cpu);
                    let r = cpu.a.wrapping_sub(m);
                    set(&mut cpu.status, C, cpu.a >= m);
                    zn(cpu, r);
                }
                _ => {
                    let m = fetch(cpu);
                    sbc(cpu, m);
                }
            }
            cpu.pc = cpu.pc.wrapping_add(len);
            cpu.cycles = cycles;
        }
        0x08 => {
            push(cpu, cpu.status | 0x30);
            cpu.pc = cpu.pc.wrapping_add(1);
            cpu.cycles = 3;
        }
        0x00 => {
            let ret = cpu.pc.wrapping_add(2);
            push(cpu, (ret >> 8) as u8);
            push(cpu, ret as u8);
            push(cpu, cpu.status | 0x30);
            cpu.status |= I;
            cpu.pc = u16::from_le_bytes([cpu.read(0xfffe), cpu.read(0xffff)]);
            cpu.cycles = 7;
        }
        _ => return false,
    }
    true
}
