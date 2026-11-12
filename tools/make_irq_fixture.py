# Hand-assemble an IRQ-driven PSID: init installs a handler at $FFFE and starts
# CIA 1 timer A; play refuses to touch the chip until the handler has run.
import struct

ORG = 0x2000
code = {}
def put(addr, *b): code[addr] = list(b)

# --- init -------------------------------------------------------------
put(0x2000, 0x78)                                  # SEI
put(0x2001, 0xA9, 0x00, 0x8D, 0x0D, 0xDC)          # LDA #$00 ; STA $DC0D  clear flags
put(0x2006, 0xA9, 0x81, 0x8D, 0x0D, 0xDC)          # LDA #$81 ; STA $DC0D  enable TA underflow
put(0x200B, 0xA9, 0x00, 0x8D, 0x04, 0xDC)          # LDA #$00 ; STA $DC04
put(0x2010, 0xA9, 0x4C, 0x8D, 0x05, 0xDC)          # LDA #$4C ; STA $DC05  latch $4C00
put(0x2015, 0xA9, 0x00, 0x8D, 0x0E, 0xDC)          # LDA #$00 ; STA $DC0E
put(0x201A, 0xA9, 0x01, 0x8D, 0x0E, 0xDC)          # LDA #$01 ; STA $DC0E  start timer A
put(0x201F, 0xA9, 0x80, 0x8D, 0xFE, 0xFF)          # LDA #$80 ; STA $FFFE
put(0x2024, 0xA9, 0x20, 0x8D, 0xFF, 0xFF)          # LDA #$20 ; STA $FFFF  handler $2080
put(0x2029, 0x58)                                  # CLI
put(0x202A, 0x60)                                  # RTS

# --- handler: acknowledge, call the play routine, leave -----------------
put(0x2080, 0x48)                                  # PHA
put(0x2081, 0x8A, 0x48)                            # TXA ; PHA
put(0x2083, 0x98, 0x48)                            # TYA ; PHA   (0x98 is TYA; 0x9A is TXS)
put(0x2085, 0xAD, 0x0D, 0xDC)                      # LDA $DC0D  acknowledge: the read clears
put(0x2088, 0x20, 0x00, 0x21)                      # JSR $2100  the handler is the player
put(0x208B, 0x68, 0xA8, 0x68, 0xAA, 0x68)          # PLA TAY PLA TAX PLA
put(0x2090, 0x40)                                  # RTI

# --- play: called by the handler; steps a tone through an octave -------
# Frequency high byte walks $10..$1F, i.e. about 244 Hz to 488 Hz, so the
# output is audible and demonstrably changing rather than a fixed tone.
put(0x2100, 0xEE, 0x01, 0x22)                      # INC $2201
put(0x2103, 0xAD, 0x01, 0x22)                      # LDA $2201
put(0x2106, 0x29, 0x0F)                            # AND #$0F
put(0x2108, 0x09, 0x10)                            # ORA #$10
put(0x210A, 0x8D, 0x01, 0xD4)                      # STA $D401  frequency high
put(0x210D, 0xA9, 0x00, 0x8D, 0x00, 0xD4)          # LDA #$00 ; STA $D400  frequency low
put(0x2112, 0xA9, 0x21, 0x8D, 0x04, 0xD4)          # LDA #$21 ; STA $D404  gate + sawtooth
put(0x2117, 0xA9, 0x01, 0x8D, 0x17, 0xD4)          # LDA #$01 ; STA $D417  route voice 1
put(0x211C, 0xA9, 0x1F, 0x8D, 0x18, 0xD4)          # LDA #$1F ; STA $D418  low-pass, full volume
put(0x2121, 0xA9, 0x00, 0x8D, 0x05, 0xD4)          # LDA #$00 ; STA $D405  fastest attack and decay
put(0x2126, 0xA9, 0xF0, 0x8D, 0x06, 0xD4)          # LDA #$F0 ; STA $D406  full sustain, fastest release
put(0x212B, 0x60)                                  # RTS

size = 0x2200 - ORG
block = bytearray(size)
for a, b in code.items():
    block[a - ORG : a - ORG + len(b)] = bytes(b)

hdr = bytearray(128)
hdr[0:4] = b"PSID"
hdr[4:6] = (2).to_bytes(2, "big")      # version
hdr[6:8] = (128).to_bytes(2, "big")    # data offset, an absolute file position
hdr[8:10] = (ORG).to_bytes(2, "big")       # load address
hdr[10:12] = (0x2000).to_bytes(2, "big")   # init
hdr[12:14] = (0x2100).to_bytes(2, "big")   # play
hdr[14:16] = (1).to_bytes(2, "big")         # song count
hdr[16] = 1; hdr[17] = 0
hdr[32:64] = b"IRQ-DRIVEN TEST FIXTURE".ljust(32, b"\0")
assert len(hdr) == 128, "data must start at the end of the header"
open("tests/fixtures/irq.sid", "wb").write(bytes(hdr) + bytes(block))
print("wrote tests/fixtures/irq.sid", 128 + len(block), "bytes")
