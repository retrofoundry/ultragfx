# S2DEX2 console witnesses

These unmodified RDP streams came from C4 on an NTSC N64 with Expansion Pak on
2026-11-15. The harness executed OoT's `[Safe] S2DEX fifo 2.05` on the RSP:

- Text SHA1: `da45c801c30c1a7186a6f49303296ef05e32f1c2`
- Data SHA1: `29833313f25cd7251e204eb808c0c8062936bbab`

The streams repeated byte-for-byte after a reboot. They establish nothing about
any other build. No ROM, microcode or full DMEM is included.
The source packet's SHA256 is
`b92664c6f75feac4cb99a4efe100cfdd351ad171d234cdbb3f4479e5ff74c144`.
The console report recorded the commands, repeat controls and DMEM observations.

`texture_loads_match_safe_205_console_streams` compares the object commands' RDP
state and TMEM bytes with replay of the captured streams. Replay normalizes only
the two ignored opcode high bits. Both paths use identical synthetic texture
bytes at the captured addresses; this is not a console pixel comparison. The
extra syncs have no HLE state effect. In the report, `c4-double` equals `c4-block`
and `c4-zero-mask` contains two loads. The status test uses different source addresses
on successive loads to distinguish a skipped load from an identical overwrite.

C0 establishes identity A/D=00010000, B/C/X/Y=0, base scales 0400/0400, mode 0
and four zero status words, including after reset. C3 establishes that DC070002
replaces the matrix tail and DC0F0000 replaces only A–D. The matrix test uses the
reported C3 words and asserts preservation of the other fields.

C4's sid 2 and 16 performed unaligned/out-of-table DMEM writes. Their rejection
is an ultragfx support limit. C1's D9 aliases the sprite handler; D7 depends on
microcode relocation and can hang. Both remain unsupported here. C2's DC080008
writes DMEM 0..15 and remains unsupported. Other matrix DMA sizes remain unsupported.

## Rectangle fixtures

The C5, C6, mode-addendum, copy-tile, C8 and reciprocal-probe rectangle streams
use the same microcode hashes above. Their `.rdp.bin` files are unmodified
console output from the `s2dex2-rectangles` clean-room evidence set. The `.input.bin` files
contain task data pages, with the display list at offset `0x1000` and physical
pointers relocated to the base given in the test.

The copy-tile, C8-pairs and reciprocal-probe input pages are copies of the captured
relocated pages. The other input pages are reconstructed from the evidence's
`PAGE-LAYOUT.md`, JSONL struct overrides, and `cases.json` command lists, using
the addresses in `relocated.json`. Unused page cells need not match the original
upload. No ROM, microcode, DMEM dump or framebuffer is included.

`object_rectangles_match_console_rdp_state` compares every emitted rectangle's
tile descriptor, tile size, selected tile, corners, texture start and steps
against replay of the console stream. It also compares final RDP state and
TMEM. Replay restores the ignored opcode high bits and wraps each rectangle's
second RDP packet in the GBI `E1`/`F1` continuation commands; it does not alter
the rectangle data. These are command-state checks, not framebuffer goldens.

The test also generates variants with the low three object-mode bits set and
with 1-cycle/2-cycle selected. These are derived checks, not additional console
captures. The two-cycle variants initialize secondary tiles 1 and 3 identically
on both paths. The fixture set covers all sixteen combinations of mode bits
`0x78`; the variants cover all 128 low-seven-bit modes in both cycles.
Their basis is the mode-selection masks and copy-bit test in the disassembly,
listed below. They assert unchanged rectangle fields, not unchanged rendered
pixels when the RDP cycle type changes.

## Rectangle derivation

The clean-room inputs are the console streams and the supplied disassembly of
the microcode identified above. Addresses below refer to that listing, including
its overlays. No other renderer's arithmetic was used for the rectangle handler.

| Behavior | Disassembly | Distinguishing console rows |
| --- | --- | --- |
| Mode constants and position rounding | `2180–21bc`, data `214–260`, `1bb8–1bc0` | `c5-objx-161/162/163`, shrink/widen and mode-addendum rows |
| Low three object-mode bits do not affect rectangle fields: selection masks are `0x08`, `0x18`, `0x70`; the rectangle path consumes the selected constants, not the stored mode byte | `2184–218c`, `219c–21bc`, `1ba0–1bac`, `1bfc–1c00` | Disassembly-derived variants, not separate captures |
| 1-cycle and 2-cycle produce the same rectangle fields: the cycle check masks only byte `0xc9` with `0x20` (the copy bit), which is clear in both | `1cb8–1ccc` | Disassembly-derived variants, not separate captures |
| RECT_R translation, BaseScale and endpoint fractional carry; A–D unused | `1d90–1df8` | C6 scale grid and asymmetric rows |
| Image-axis flips and texture starts | `1c04–1c34` | `c5-both-flips`, shrink/bilerp rows |
| Scissor clipping and fractional correction on T only | `1c38–1c88` | `c5-bilerp-scale-0555`, `c6-asymmetric-mode-08`, `rect-r-recip-identity-negative` |
| Default scissor is `[0, 0, 1280, 960]` in 10.2 units: initial data at `204–20b` is `0000 0500 0000 03c0` in X-start/end, Y-start/end order | Data `204–20b`; scissor writes `11dc–1204`; rectangle loads `1c38–1c3c` | Initial microcode data and disassembly, not a no-scissor console row |
| Draw tile toggles 0/2, without resetting on a texture load | `1cd0–1cd8`, initial data `2af` | `c8-four-same`, `c8-load-draw-load` |
| Descriptor and packed tile-size fields | `1cf0–1d60` | `c5-ci4`, `c5-imagew-1000` |
| Copy horizontal step ×4 and packed far corner minus `0x4004` | `1cb8–1ccc` | `c5-copy` |
| Combined transfer: 24-byte descriptor followed by 24-byte sprite | `1550–163c`, data `20c–212` | `c8-rect-combined`, `c8-rectR-combined`, C8 size-47 rows |

`object_reciprocal` keeps the denominator's leading ten bits, divides `2^31`
by that denominator, then adds the quotient's high half. The high-half addition
comes from `1bdc–1be0` and `1db8–1dc0`. Full division fails
`rect-recip-0555-edge`. Discarding only one bit fails `rect-recip-0802-lsb`
and `rect-recip-1004-lsb`; `rect-recip-0804-lsb` distinguishes the precision
boundary. `rect-r-recip-0802-position` checks the same distinction in transformed
positions. This rule matches the exposed rectangle fields; the captures do not
directly measure every bit of the RSP reciprocal register.

Zero and nonpositive signed-16-bit reciprocal inputs remain unsupported. Combined
size-23 commands also remain unsupported: C8 shows that they reuse an earlier
sprite cell, whose lifetime overlaps other object/background scratch data.
Rotated object sprites remain unsupported. Background handling is unchanged.
