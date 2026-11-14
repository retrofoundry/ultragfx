Raw big-endian RGBA16 words captured from an NTSC N64 on 2026-11-06.
Each threshold file is 128x4 pixels.
Every four columns use one constant primitive RGB value: 0..15, then 240..255.
Alpha is 255, CVG_DST_FULL, alpha dither disabled. Each block covers the full
4x4 RGB dither cell. Files pin magic (selector 0), Bayer (1) and disable (3).
Noise (2) deliberately uses the magic fixture; the hardware RNG is unknown.


`last-writer.rgba16` is a 4x4 (19,85,141) source with magic dither, column 1
then overwritten with dither disabled, followed by a Bayer SetOtherMode.
The first word is `0x1ae5`; column 1 is `0x12a3`. A second capture
repeated all three threshold fixtures with identical CRCs.

`tex-persp.rgba16` is eight 64x32 regions, cases A to H in order, from a 320x240 framebuffer
captured on 2026-11-14. Two runs were byte-identical (full framebuffer sha1
2cca9aed791da2baaf16b64ab8a23a11a45c61a8), and every pixel outside the regions was the clear
colour 0x001F. The display list ran on OoT NTSC 1.0's
`RSP Gfx ucode F3DZEX.NoN  fifo 2.06H Yoshitaka Yasumoto 1998 Nintendo.` (text sha1
d6f36e2d202aa7d5b65543cc6b7cd77eab12831c, data sha1 c672b618600753add498460a8893dc9d10da0b4b),
not the F3DEX fifo 2.05 build of issue #153. Its words are in
`guest_bytes_tp_none_triangles_sample_halved_screen_linear_coordinates`.

The texture is 64x32 RGBA16 whose texel (s, t) holds R = s & 31, G = t, B = 31 when s >= 32 and
alpha 1. It is sampled with wrap, masks 6 and 5, point filtering, 1-cycle, combiner TEXEL0,
colour dither off and OPA_SURF, with no Z and no geometry mode, gSPTexture 0xFFFF and
perspNorm 0xFFFF. The viewport is scale 512, 512 and translation 640, 480 in quarter pixels.
The ortho projection maps X = x - 160, Y = 120 - y at W 1; the perspective projection gives
screen x = 160 + 128 X / Z with W = Z / 4. Each quad lists TL, BL, BR, TR with S 0 to 1024 and
T 0 to 512 (32 by 16 texels) and draws triangles (0, 1, 2) and (0, 2, 3) unless noted.

| Case | Region | Mode | Geometry |
| --- | --- | --- | --- |
| A | 40, 16 | G_TP_PERSP | ortho |
| B | 128, 16 | G_TP_NONE | ortho |
| C | 216, 16 | G_TP_PERSP | Z 256 at the left edge (32 rows), 768 at the right |
| D | 40, 64 | G_TP_NONE | as C |
| E | 128, 64 | G_TP_NONE | texture rectangle, dsdx = dtdy = 0x200 |
| F | 216, 64 | G_TP_NONE | B with SetTileSize uls 8, ult 4 |
| G | 40, 112 | G_TP_NONE | perspective, every vertex at Z 400 |
| H | 128, 112 | G_TP_NONE | screen rectangle, Z 256 at TL and 768 elsewhere, triangles (0, 1, 3) and (1, 2, 3) |

They were drawn in the order A, B, E, F, D, G, H, C. G_TP_NONE triangles sample half of S/W
divided by the triangle's largest 1/W, interpolated linearly across the screen; F shows the
halving comes before the tile origin, and E that rectangles are not halved.
`guest_bytes_tp_none_triangles_sample_halved_screen_linear_coordinates` compares, at 1x, 2x and
4x, the interior of H and of the rectangle control E whole and the S fields (mask 0xF83E) of B and
F, where interior leaves out pixels within 1 px of the region border or of the diagonal the case's
triangles share (TL to BR for E).
