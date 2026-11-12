//! Clean-room, see mos6581's docs/provenance.md.
//! Clean-room C64 RAM image for `$0000-$03FF` at the READY prompt after
//! KERNAL reset and BASIC cold start.
//!
//! Sources (abbreviations used below):
//! * PT = pagetable.com c64mem memory map (<https://www.pagetable.com/c64ref/c64mem/>)
//! * MAP = Compute!'s "Mapping the Commodore 64" (archive.org item
//!   `Compute_s_Mapping_the_Commodore_64`, full text)
//!
//! Anything not documented stays zero. See REPORT.md.

const SIZE: usize = 0x400;

const fn set8(mut m: [u8; SIZE], addr: usize, v: u8) -> [u8; SIZE] {
    m[addr] = v;
    m
}

/// Store a 16-bit value low byte first.
const fn set16(mut m: [u8; SIZE], addr: usize, v: u16) -> [u8; SIZE] {
    m[addr] = (v & 0xFF) as u8;
    m[addr + 1] = (v >> 8) as u8;
    m
}

const fn build() -> [u8; SIZE] {
    let mut m = [0u8; SIZE];

    // $00 D6510. Source: PT $0000 lists $EF in one source and $2F in another.
    // Own choice: $2F (bits 0-5 significant, per PT, so bits 6-7 clear).
    m = set8(m, 0x00, 0x2F);
    // $01 R6510. Source: PT $0001 "Default $37 (%00110111)".
    m = set8(m, 0x01, 0x37);

    // $03-$04 ADRAY1. Source: PT $0003-$0004 default $B1AA.
    m = set16(m, 0x03, 0xB1AA);
    // $05-$06 ADRAY2. Source: PT $0005-$0006 default $B391.
    m = set16(m, 0x05, 0xB391);

    // $13 CHANNL. Source: PT $0013 "Default $00 (keyboard input, screen output)".
    m = set8(m, 0x13, 0x00);
    // $16 TEMPPT. Source: PT $0016 default $19; MAP 22 "points to 25 ($19)
    // when the stack is empty".
    m = set8(m, 0x16, 0x19);
    // $17-$18 LASTPT. Source: PT $0017-$0018 "byte 23 is 3 less than byte 22;
    // byte 24 is 0". Derived: $16, $00.
    m = set16(m, 0x17, 0x0016);

    // $2B-$2C TXTTAB. Source: PT $002B and MAP 43-44, default $0801.
    m = set16(m, 0x2B, 0x0801);
    // $2D-$2E VARTAB, $2F-$30 ARYTAB, $31-$32 STREND. Sources say only that
    // VARTAB is "end of program + 1" (PT, MAP). Derived: empty program means
    // end+1 = $0803 (three zero bytes at $0801-$0803 are outside this image).
    // ARYTAB/STREND set equal to VARTAB as for an empty variable area (derived).
    m = set16(m, 0x2D, 0x0803);
    m = set16(m, 0x2F, 0x0803);
    m = set16(m, 0x31, 0x0803);
    // $33-$34 FRETOP. Source: PT $0033 "Power-on: $A000".
    m = set16(m, 0x33, 0xA000);
    // $37-$38 MEMSIZ. PT $0037 and MAP 55-56 say $9FFF (highest byte); PT
    // $0283 says $A000 (end of user RAM). Own choice: $A000, consistent with
    // FRETOP and the "CLR sets FRETOP to MEMSIZ" statement in PT/MAP.
    m = set16(m, 0x37, 0xA000);
    // $39-$3A CURLIN. Source: PT $0039 "Set to 0,255 for direct statements".
    m = set16(m, 0x39, 0xFF00);
    // $41-$42 DATPTR. PT $0041 sources conflict (same as 43-44 vs. the zero
    // before the start of text). Own choice: $0800.
    m = set16(m, 0x41, 0x0800);

    // $73-$8A CHRGET. Source: PT $0073-$008A mnemonic listing (INC $7A; BNE
    // $79; INC $7B; LDA $0801; CMP #$3A; BCS $8A; CMP #$20; BEQ $73; SEC;
    // SBC #$30; SEC; SBC #$D0; RTS). Opcode byte encodings are NOT in the
    // permitted sources; they come from the general 6502 instruction set.
    let chrget: [u8; 24] = [
        0xE6, 0x7A, 0xD0, 0x02, 0xE6, 0x7B, 0xAD, 0x07, 0x02, 0xC9, 0x3A, 0xB0, 0x0A, 0xC9, 0x20,
        0xF0, 0xEF, 0x38, 0xE9, 0x30, 0x38, 0xE9, 0xD0, 0x60,
    ];
    let mut i = 0;
    while i < chrget.len() {
        m[0x73 + i] = chrget[i];
        i += 1;
    }
    // $7A-$7B TXTPTR (LDA operand inside CHRGET). PT shows $0801 in the raw
    // listing and $0207 in the annotated one. Own choice: $0207, a position in
    // the input buffer, as at READY in direct mode.
    m = set16(m, 0x7A, 0x0207);

    // $8B-$8F RNDX. Source: PT $008B and MAP 139-143: 128,79,199,82,88.
    m[0x8B] = 0x80;
    m[0x8C] = 0x4F;
    m[0x8D] = 0xC7;
    m[0x8E] = 0x52;
    m[0x8F] = 0x58;

    // $91 STKEY. Source: PT $0091 "$FF = no key pressed".
    m = set8(m, 0x91, 0xFF);
    // $95 BSOUR. Source: PT $0095 "$FF = nothing waiting". Derived.
    m = set8(m, 0x95, 0xFF);
    // $99 DFLTN = 0 keyboard, $9A DFLTO = 3 screen. Source: PT $0099, $009A.
    m = set8(m, 0x99, 0x00);
    m = set8(m, 0x9A, 0x03);
    // $9D MSGFLG. Source: PT $009D "128 in direct mode".
    m = set8(m, 0x9D, 0x80);
    // $A0-$A2 TIME. Source: PT $00A0 "Starts at 0 at power-on". Zero.

    // $B2-$B3 TAPE1. Source: MAP 178-179 "set to ... cassette buffer (828, $33C)".
    m = set16(m, 0xB2, 0x033C);
    // $C5 LSTX and $CB SFDX. Source: PT $00C5, $00CB "64 = none".
    m = set8(m, 0xC5, 0x40);
    m = set8(m, 0xCB, 0x40);
    // $D5 LNMX. Source: PT $00D5 "$27 or $4F". Derived: $27 (40-column line).
    m = set8(m, 0xD5, 0x27);

    // $D9-$F2 LDTB1 line link table. Source: PT $00D9 (25 entries; bits 0-3
    // screen page; bit 7 set for first/only physical line) and MAP 217-242.
    // Derived: every row a single physical line at $0400+40*row, so
    // $80 | high byte of the row address.
    let mut row = 0;
    while row < 25 {
        let page = ((0x0400 + 40 * row) >> 8) as u8;
        m[0xD9 + row] = 0x80 | (page & 0x0F);
        row += 1;
    }

    // $F5-$F6 KEYTAB. Source: PT $00F5, MAP 245-246: $EB81.
    m = set16(m, 0xF5, 0xEB81);

    // $0281-$0282 MEMSTR. Source: PT $0281 "$0800".
    m = set16(m, 0x281, 0x0800);
    // $0283-$0284 MEMSIZ. Source: PT $0283 "$A000".
    m = set16(m, 0x283, 0xA000);
    // $0288 HIBASE. Source: PT $0288 $04; MAP 648.
    m = set8(m, 0x288, 0x04);
    // $0289 XMAX. Source: PT $0289, MAP 649: 10.
    m = set8(m, 0x289, 10);
    // $028A RPTFLA. Source: PT $028A, MAP 650: 0.
    m = set8(m, 0x28A, 0);
    // $028B KOUNT. PT: text 6, symbol table $04. Own choice: 6 (MAP 651).
    m = set8(m, 0x28B, 6);
    // $028C DELAY. Source: PT $028C: 16.
    m = set8(m, 0x28C, 16);
    // $028F-$0290 KEYLOG. Source: PT $028F: $EB48.
    m = set16(m, 0x28F, 0xEB48);
    // $0291 MODE. Source: PT $0291: $00.
    m = set8(m, 0x291, 0);

    // $0300-$030B BASIC vectors. Source: PT $0300-$030B.
    m = set16(m, 0x300, 0xE38B); // IERROR
    m = set16(m, 0x302, 0xA483); // IMAIN
    m = set16(m, 0x304, 0xA57C); // ICRNCH
    m = set16(m, 0x306, 0xA71A); // IQPLOP
    m = set16(m, 0x308, 0xA7E4); // IGONE
    m = set16(m, 0x30A, 0xAE86); // IEVAL
    // $0310 USR opcode JMP ($4C) and $0311-$0312 USRADD. Source: PT $0310
    // note ($4C) and its note that the target is the USR function vector
    // $B248 (ILLEGAL QUANTITY routine, PT $0311 text).
    m = set8(m, 0x310, 0x4C);
    m = set16(m, 0x311, 0xB248);

    // $0314-$0333 KERNAL vectors. Source: PT $0314-$0333.
    m = set16(m, 0x314, 0xEA31); // CINV
    m = set16(m, 0x316, 0xFE66); // CBINV
    m = set16(m, 0x318, 0xFE47); // NMINV
    m = set16(m, 0x31A, 0xF34A); // IOPEN
    m = set16(m, 0x31C, 0xF291); // ICLOSE
    m = set16(m, 0x31E, 0xF20E); // ICHKIN
    m = set16(m, 0x320, 0xF250); // ICKOUT
    m = set16(m, 0x322, 0xF333); // ICLRCH
    m = set16(m, 0x324, 0xF157); // IBASIN
    m = set16(m, 0x326, 0xF1CA); // IBSOUT
    m = set16(m, 0x328, 0xF6ED); // ISTOP
    m = set16(m, 0x32A, 0xF13E); // IGETIN
    m = set16(m, 0x32C, 0xF32F); // ICLALL
    m = set16(m, 0x32E, 0xFE66); // USRCMD
    // PT gives two values each for LOAD/SAVE ($F49E/$F4A5, $F5DD/$F5ED).
    // Own choice: the "default" entries $F4A5 and $F5ED.
    m = set16(m, 0x330, 0xF4A5); // ILOAD
    m = set16(m, 0x332, 0xF5ED); // ISAVE

    m
}

/// RAM contents `$0000-$03FF` at the READY prompt.
pub const POWERON: [u8; 0x400] = build();

#[cfg(test)]
mod tests {
    use super::POWERON;

    fn w(a: usize) -> u16 {
        u16::from(POWERON[a]) | (u16::from(POWERON[a + 1]) << 8)
    }
    fn basic(v: u16) -> bool {
        (0xA000..=0xBFFF).contains(&v)
    }
    fn kernal(v: u16) -> bool {
        v >= 0xE000
    }

    #[test]
    fn size_and_port() {
        assert_eq!(POWERON.len(), 0x400);
        assert_eq!(POWERON[0x01] & 0x07, 0x07); // LORAM/HIRAM/CHAREN set
        assert_eq!(POWERON[0x00] & 0xC0, 0); // only bits 0-5 significant
    }

    #[test]
    fn basic_vectors_in_roms() {
        assert!(kernal(w(0x300)));
        for a in (0x302..=0x30A).step_by(2) {
            assert!(basic(w(a)), "{a:#x}");
        }
        assert!(basic(w(0x03)) && basic(w(0x05)));
        assert_eq!(POWERON[0x310], 0x4C);
        assert!(basic(w(0x311)));
    }

    #[test]
    fn kernal_vectors_in_rom() {
        for a in (0x314..=0x332).step_by(2) {
            assert!(kernal(w(a)), "{a:#x}");
        }
        assert_eq!(POWERON[0x315], 0xEA); // low byte first
        assert_eq!(w(0x316), w(0x32E)); // USRCMD points at BRK routine
        assert!(kernal(w(0xF5)) && kernal(w(0x28F)));
    }

    #[test]
    fn memory_pointers_consistent() {
        assert_eq!(w(0x2B), 0x0801);
        assert!(w(0x2D) >= w(0x2B) && w(0x2F) >= w(0x2D) && w(0x31) >= w(0x2F));
        assert_eq!(w(0x33), w(0x37));
        assert_eq!(w(0x37), w(0x283));
        assert_eq!(w(0x281) + 1, w(0x2B));
        assert_eq!(w(0xB2), 0x033C);
        assert_eq!(usize::from(POWERON[0x288]) << 8, 0x0400);
    }

    #[test]
    fn chrget_shape() {
        assert_eq!(POWERON[0x73], 0xE6); // INC zp
        assert_eq!(POWERON[0x8A], 0x60); // RTS
        let t = w(0x7A);
        assert!(t >= 0x0200);
        // LDA abs operand slot lies within the copied routine bytes
        assert_eq!(POWERON[0x79], 0xAD);
    }

    #[test]
    fn line_link_table() {
        for row in 0..25usize {
            let b = POWERON[0xD9 + row];
            let page = usize::from(b & 0x0F);
            assert_eq!(page, (0x0400 + 40 * row) >> 8);
            assert_ne!(b & 0x80, 0);
        }
        assert_eq!(POWERON[0xD5], 0x27);
    }

    #[test]
    fn keyboard_defaults() {
        assert_eq!(POWERON[0x289], 10);
        assert_eq!(POWERON[0x28A], 0);
        assert_eq!((POWERON[0x99], POWERON[0x9A]), (0, 3));
    }
}
