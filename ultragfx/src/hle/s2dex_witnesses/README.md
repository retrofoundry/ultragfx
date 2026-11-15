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
writes DMEM 0..15 and remains unsupported. Other DMA sizes await C8.
