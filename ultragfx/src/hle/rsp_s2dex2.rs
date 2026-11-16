/*
MIT License

Copyright (c) 2024 RT64 Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/

use super::interp::{checked_span, emit_texrect, memory_try, Cmd, Ctx, Handler, TexRect};
use super::mem::Rdram;
use super::mem::{ObjBg, ObjBgKind, ObjMtx, ObjMtxKind, ObjSprite};
use super::rdp::{load_tile_typed, set_tile_size_typed, set_tile_typed, TileDescriptor};
use crate::inspect::Emission;
use crate::{DiagKind, Diagnostic};
use n64_gbi::consts::{rdp, rsp_s2dex2::*};

pub(crate) fn install<M: Rdram>(table: &mut [Handler<M>; 256]) {
    table.fill(unsupported::<M>);
    super::rdp::install_defaults(table);
    table[G_OBJ_LOADTXTR as usize] = obj_load_txtr;
    table[G_OBJ_MOVEMEM as usize] = obj_move_mem;
    table[0x0b] = obj_render_mode;
    table[0xdb] = move_word;
    table[0xe0] = super::rsp_f3dex2::spnoop;
    table[0xe2] = super::rsp_f3dex2::set_other_mode_l;
    table[0xe3] = super::rsp_f3dex2::set_other_mode_h;
    table[0xfd] = super::rsp_f3dex2::set_texture_image;
}

fn obj_render_mode<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    cx.rsp.obj_render_mode = c.w1;
}

fn object_reciprocal(scale: u16) -> Option<i64> {
    if scale == 0 || scale > i16::MAX as u16 {
        return None;
    }
    let shift = (16 - scale.leading_zeros()).saturating_sub(10);
    let denominator = (scale >> shift) << shift;
    let reciprocal = (1i64 << 31) / i64::from(denominator);
    Some(reciprocal + (reciprocal >> 16))
}

fn object_rectangle(
    sprite: &ObjSprite,
    matrix: Option<&ObjMtx>,
    mode: u32,
    scissor: [u16; 4],
) -> Result<Option<TexRect>, ()> {
    let (offset, subtraction) = [
        (0, 0),
        (16, 32),
        (32, 64),
        (48, 96),
        (0, -12),
        (16, 20),
        (32, 52),
        (48, 84),
    ][((mode >> 4) & 7) as usize];
    let bias = [1, -2, 1, 0][((mode >> 3) & 3) as usize];
    let bilerp = mode & 8 != 0;
    let mask = if bilerp { -1 } else { -4 };
    let mut corners = [[0; 2]; 2];
    let mut starts = [0; 2];
    let mut steps = [0; 2];
    for (axis, (position, scale, size, flip)) in [
        (sprite.obj_x, sprite.scale_w, sprite.image_w, 1),
        (sprite.obj_y, sprite.scale_h, sprite.image_h, 0x10),
    ]
    .into_iter()
    .enumerate()
    {
        let mut step = i64::from(scale);
        let (mut start, fraction) = if let Some(matrix) = matrix {
            let (translation, base) = if axis == 0 {
                (matrix.x, matrix.base_scale_x)
            } else {
                (matrix.y, matrix.base_scale_y)
            };
            let fixed = (i64::from(position) * 2048 * object_reciprocal(base).ok_or(())?) >> 16;
            step = (step * i64::from(base)) >> 10;
            (
                (fixed >> 16) + ((i64::from(translation) + bias) & mask),
                fixed & 0xffff,
            )
        } else {
            ((i64::from(position) + bias) & mask, 0)
        };
        let extent = ((i64::from(size) - subtraction)
            * 256
            * object_reciprocal(u16::try_from(step).map_err(|_| ())?).ok_or(())?)
            >> 16;
        let end = (start + ((fraction + extent) >> 16)).min(i64::from(scissor[axis + 2]));
        let mut texture = if sprite.image_flags & flip != 0 {
            step = -step;
            i64::from(size) - subtraction + offset - 1
        } else {
            offset
        };
        if bilerp {
            texture -= 16;
        }
        let clipped = (i64::from(scissor[axis]) - start).max(0);
        texture += (clipped * step) >> 7;
        start += clipped;
        if start >= end {
            return Ok(None);
        }
        if bilerp && axis == 1 {
            texture -= (step * (start & 3)) >> 7;
        }
        corners[axis] = [start as i32, end as i32];
        starts[axis] = texture as i16;
        steps[axis] = step as i16;
    }
    Ok(Some(TexRect {
        rect: crate::scene::TexRectBounds {
            ulx: corners[0][0],
            uly: corners[1][0],
            lrx: corners[0][1],
            lry: corners[1][1],
        },
        tile: 0,
        uls: starts[0],
        ult: starts[1],
        dsdx: steps[0],
        dtdy: steps[1],
        flip: false,
    }))
}

pub(crate) fn obj_rectangle<M: Rdram>(
    c: &Cmd,
    cx: &mut Ctx<M>,
) -> Result<Vec<Emission<'static>>, ()> {
    let cycle = (cx.rdp.other_mode_h >> 20) & 3;
    let combined = matches!(c.opcode(), 0x07 | 0x08);
    if combined && c.p0(0, 24) != 47 {
        invalid_parameters(cx, c.opcode());
        return Err(());
    }
    if cycle == 3 {
        unsupported(c, cx);
        return Err(());
    }
    let sprite = cx
        .mem
        .resolve_masked(c.w1_addr)
        .and_then(|address| checked_span(address, if combined { 24 } else { 0 }))
        .and_then(|address| cx.mem.read_obj_sprite(address))
        .map_err(|error| {
            cx.diags.push(Diagnostic {
                at: cx.pc,
                kind: DiagKind::MemoryRead {
                    access: crate::MemoryAccess::ObjSprite,
                    error,
                },
            })
        })?;
    if combined {
        let diagnostics = cx.diags.len();
        obj_load_txtr(
            &Cmd {
                w0: (c.w0 & 0xff00_0000) | 23,
                w1: c.w1,
                w1_addr: c.w1_addr,
            },
            cx,
        );
        if cx.diags.len() != diagnostics {
            return Err(());
        }
    }
    let rectangle = object_rectangle(
        &sprite,
        matches!(c.opcode(), 0x08 | 0xda).then_some(&cx.rsp.obj_mtx),
        cx.rsp.obj_render_mode,
        cx.rdp.raw_scissor.unwrap_or([0, 0, 1280, 960]),
    )
    .map_err(|()| unsupported(c, cx))?;
    let Some(mut rectangle) = rectangle else {
        return Ok(Vec::new());
    };
    if cycle == 2 {
        rectangle.dsdx = rectangle.dsdx.wrapping_mul(4);
        let corner = ((rectangle.rect.lrx as u32) << 12) | rectangle.rect.lry as u32;
        let corner = corner.wrapping_sub(0x4004);
        rectangle.rect.lrx = ((corner >> 12) & 0xfff) as i32;
        rectangle.rect.lry = (corner & 0xfff) as i32;
    }
    cx.rsp.obj_tile ^= 2;
    rectangle.tile = cx.rsp.obj_tile;
    let w0 = (u32::from(sprite.image_fmt) << 21)
        | (u32::from(sprite.image_siz) << 19)
        | (u32::from(sprite.image_stride) << 9)
        | u32::from(sprite.image_adrs);
    let w1 = 0x0008_0200
        | ((u32::from(sprite.image_pal) << 20) & 0xffffff)
        | (u32::from(rectangle.tile) << 24);
    let size = ((u32::from(sprite.image_w & 0x7ff8) << 9) | (u32::from(sprite.image_h) >> 3))
        .wrapping_sub(0x4004);
    super::rdp::set_tile(
        &Cmd {
            w0,
            w1,
            w1_addr: u64::from(w1),
        },
        cx,
    );
    set_tile_size_typed(
        usize::from(rectangle.tile),
        [0, 0, ((size >> 12) & 0xfff) as u16, (size & 0xfff) as u16],
        cx,
    );
    emit_texrect(rectangle, cx)
        .map(|emission| vec![emission])
        .ok_or(())
}

fn move_word<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    if c.p0(16, 8) == u32::from(G_MW_GENSTAT) {
        if let Some(index) = status_index(c.w0 as u16, cx) {
            cx.rsp.obj_status[index] = c.w1;
        }
    } else {
        super::rsp_f3dex2::move_word(c, cx);
    }
}

fn status_index<M: Rdram>(sid: u16, cx: &mut Ctx<M>) -> Option<usize> {
    if matches!(sid, 0 | 4 | 8 | 12) {
        Some(usize::from(sid / 4))
    } else {
        cx.diags.push(Diagnostic {
            at: cx.pc,
            kind: DiagKind::UnsupportedObjStatus { sid },
        });
        None
    }
}

fn obj_load_txtr<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    if c.p0(0, 24) != 23 {
        invalid_parameters(cx, c.opcode());
        return;
    }
    let obj = memory_try!(
        cx,
        ObjTxtr,
        cx.mem
            .resolve_masked(c.w1_addr)
            .and_then(|address| cx.mem.read_obj_txtr(address))
    );
    let Some(index) = status_index(obj.sid, cx) else {
        return;
    };
    let status = cx.rsp.obj_status[index];
    if status & obj.mask == obj.flag {
        return;
    }
    let opcode = (obj.load_type as u8 & 0x3f) | 0xc0;
    let load: Handler<M> = match opcode {
        rdp::G_LOADBLOCK => super::rdp::load_block,
        rdp::G_LOADTILE => super::rdp::load_tile,
        rdp::G_LOADTLUT => super::rdp::load_tlut,
        _ => {
            invalid_parameters(cx, c.opcode());
            return;
        }
    };
    cx.rsp.obj_status[index] = (status & !obj.mask) | (obj.flag & obj.mask);
    let image = memory_try!(cx, Texture, cx.mem.resolve(obj.image));
    cx.rsp
        .set_texture_image(0, 2, obj.tsize & 0xfff, image, cx.rdp);

    // RT64's packing, checked against the Safe 2.05 C4 streams in s2dex_witnesses/.
    let type_mask = (obj.load_type >> 16) as i8 as u32;
    let tile = u32::from(obj.tmem)
        | (((u32::from(obj.tsize) + 1) & type_mask) << 7)
        | ((obj.load_type & 0xff00) << 8);
    super::rdp::set_tile(
        &Cmd {
            w0: (u32::from(rdp::G_SETTILE) << 24) | tile,
            w1: 0x0700_0000,
            w1_addr: 0x0700_0000,
        },
        cx,
    );
    let w1 = 0x0700_0000 | (((u32::from(obj.tsize) << 14) | u32::from(obj.tline)) & 0xffffff);
    load(
        &Cmd {
            w0: u32::from(opcode) << 24,
            w1,
            w1_addr: u64::from(w1),
        },
        cx,
    );
}

fn obj_move_mem<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    let kind = match c.p0(0, 24) {
        0x170000 => ObjMtxKind::Full,
        0x070002 => ObjMtxKind::Sub,
        0x0f0000 => ObjMtxKind::Linear,
        _ => {
            invalid_parameters(cx, c.opcode());
            return;
        }
    };
    let mtx = memory_try!(
        cx,
        ObjMtx,
        cx.mem
            .resolve_masked(c.w1_addr)
            .and_then(|address| cx.mem.read_obj_mtx(address, kind))
    );
    let state = &mut cx.rsp.obj_mtx;
    if kind != ObjMtxKind::Sub {
        state.a = mtx.a;
        state.b = mtx.b;
        state.c = mtx.c;
        state.d = mtx.d;
    }
    if kind != ObjMtxKind::Linear {
        state.x = mtx.x;
        state.y = mtx.y;
        state.base_scale_x = mtx.base_scale_x;
        state.base_scale_y = mtx.base_scale_y;
    }
}

fn unsupported<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) {
    cx.diags.push(Diagnostic {
        at: cx.pc,
        kind: DiagKind::UnsupportedCommand {
            opcode: c.opcode(),
            w0: c.w0,
            w1: c.w1_addr,
        },
    });
}

fn invalid_parameters<M: Rdram>(cx: &mut Ctx<M>, opcode: u8) {
    cx.diags.push(Diagnostic {
        at: cx.pc,
        kind: DiagKind::UnsupportedCommandParameters { opcode },
    });
}

fn read_bg<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>, kind: ObjBgKind) -> Result<ObjBg, ()> {
    let result = cx
        .mem
        .resolve_masked(c.w1_addr)
        .and_then(|address| cx.mem.read_obj_bg(address, kind));
    let bg = result.map_err(|error| {
        cx.diags.push(Diagnostic {
            at: cx.pc,
            kind: DiagKind::MemoryRead {
                access: crate::MemoryAccess::ObjBg,
                error,
            },
        })
    })?;
    if bg.image_w == 0
        || bg.image_h == 0
        || bg.frame_w == 0
        || bg.frame_h == 0
        || bg.image_siz > 3
        || bg.image_fmt > 4
        || !matches!(bg.image_load, 0x0033 | 0xfff4)
    {
        invalid_parameters(cx, c.opcode());
        return Err(());
    }
    Ok(bg)
}

fn source_address<M: Rdram>(cx: &mut Ctx<M>, address: u64, offset: i64) -> Result<u64, ()> {
    address.checked_add_signed(offset).ok_or_else(|| {
        cx.diags.push(Diagnostic {
            at: cx.pc,
            kind: DiagKind::MemoryRead {
                access: crate::MemoryAccess::Texture,
                error: crate::MemoryError {
                    address,
                    length: offset.unsigned_abs(),
                    kind: crate::MemoryErrorKind::AddressOverflow,
                },
            },
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn load_1cyc_tile<M: Rdram>(
    cx: &mut Ctx<M>,
    bg: &ObjBg,
    pointer: u64,
    stride: i64,
    rows: i64,
    line: i64,
    destination: i64,
    width: i64,
    set_tile: bool,
) -> Result<(), ()> {
    if rows <= 0 || width <= 0 || destination < 0 {
        invalid_parameters(cx, 0x09);
        return Err(());
    }
    let size = if bg.image_siz == 3 { 3 } else { 2 };
    if set_tile {
        set_tile_typed(
            7,
            TileDescriptor {
                fmt: 0,
                siz: size,
                line: line as u16,
                tmem_addr: destination as u16,
                ..Default::default()
            },
            cx,
        );
    }
    cx.rsp
        .set_texture_image(0, size, (stride / 2 - 1) as u16, pointer, cx.rdp);
    load_tile_typed(
        7,
        [
            0,
            0,
            ((width - 1) * 16) as u16,
            if size == 3 { 0 } else { (rows * 4 - 1) as u16 },
        ],
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn load_1cyc<M: Rdram>(
    cx: &mut Ctx<M>,
    bg: &ObjBg,
    top: u64,
    pointer: &mut u64,
    remain: &mut i64,
    stride: i64,
    x_offset: i64,
    source_rows: i64,
    line: i64,
    draw_rows: i64,
    bilerp: i64,
    split: i64,
) -> Result<(), ()> {
    let load_rows = draw_rows + bilerp;
    let loadable = *remain - split;
    if loadable >= load_rows {
        load_1cyc_tile(cx, bg, *pointer, stride, load_rows, line, 0, line, false)?;
        *pointer = source_address(cx, *pointer, stride * draw_rows)?;
        *remain -= draw_rows;
        return Ok(());
    }
    let mut y2 = *remain;
    let mut l2 = load_rows - y2;
    let d2 = draw_rows - y2;
    if l2 > 0 {
        let mut p2 = source_address(cx, top, x_offset)?;
        if y2 & 1 != 0 {
            p2 = source_address(cx, p2, -stride)?;
            y2 -= 1;
            l2 += 1;
        }
        load_1cyc_tile(cx, bg, p2, stride, l2, line, y2 * line, line, true)?;
    }
    if split != 0 {
        let mut p1a = source_address(cx, *pointer, loadable * stride)?;
        let mut p1b = top;
        let mut y1 = loadable;
        let mut l1 = loadable & 1;
        if l1 != 0 {
            p1a = source_address(cx, p1a, -stride)?;
            p1b = source_address(cx, p1b, -stride)?;
            y1 -= 1;
        }
        l1 += 1;
        let a = (stride - x_offset) / 8;
        let b = line - a;
        load_1cyc_tile(cx, bg, p1b, stride, l1, line, y1 * line + a, b, true)?;
        load_1cyc_tile(cx, bg, p1a, stride, l1, line, y1 * line, a, true)?;
    }
    if loadable > 0 {
        load_1cyc_tile(cx, bg, *pointer, stride, loadable, line, 0, line, true)?;
    } else {
        set_tile_typed(
            7,
            TileDescriptor {
                siz: if bg.image_siz == 3 { 3 } else { 2 },
                line: line as u16,
                ..Default::default()
            },
            cx,
        );
    }
    *remain -= draw_rows;
    if *remain > 0 {
        *pointer = source_address(cx, *pointer, stride * draw_rows)?;
    } else {
        *remain = source_rows - d2;
        *pointer = source_address(cx, top, d2 * stride + x_offset)?;
    }
    Ok(())
}

pub(crate) fn bg_1cyc<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) -> Result<Vec<Emission<'static>>, ()> {
    let bg = read_bg(c, cx, ObjBgKind::Scaled)?;
    let scale_w = i64::from(bg.scale_w.max(1));
    let scale_h = i64::from(bg.scale_h.max(1));
    let max_w = ((i64::from(bg.image_w) << 10) / scale_w - 1) & !3;
    let max_h = ((i64::from(bg.image_h) << 10) / scale_h - 1) & !3;
    let removed_w = (i64::from(bg.frame_w) - max_w).max(0);
    let mut width = i64::from(bg.frame_w) - removed_w;
    let mut height = i64::from(bg.frame_h).min(max_h);
    let flip = bg.image_flip & 1 != 0;
    let mut x = i64::from(bg.frame_x) + if flip { removed_w } else { 0 };
    let mut y = i64::from(bg.frame_y);
    let scissor = cx.rdp.scissor;
    let left = (i64::from(scissor.ulx) * 4 - x).max(0);
    let top = (i64::from(scissor.uly) * 4 - y).max(0);
    let right = (width - i64::from(scissor.lrx) * 4 + x).max(0);
    let bottom = (height - i64::from(scissor.lry) * 4 + y).max(0);
    width -= left + right;
    height -= top + bottom;
    x += left;
    y += top;
    let mut emissions = Vec::new();
    if width <= 0 || height <= 0 {
        return Ok(emissions);
    }
    let x_end = x + width;
    let mut frame_y = y >> 2;
    let mut frame_remain = height >> 2;
    let source_w = i64::from(bg.image_w) * 8;
    let source_h = i64::from(bg.image_h) * 8;
    let bilerp = i64::from(cx.rsp.obj_render_mode & 8 != 0);
    let image_width = (width * scale_w) >> 7;
    let slice_width = image_width + bilerp * 32;
    let mut image_x = i64::from(bg.image_x) + (((if flip { right } else { left }) * scale_w) >> 7);
    let mut image_y = i64::from(bg.image_y) + ((top * scale_h) >> 7);
    let mut origin = i64::from(bg.image_y_orig);
    let carry = image_x / source_w;
    image_x %= source_w;
    image_y += carry * 32;
    origin += carry * 32;
    let wrap = image_y / source_h;
    image_y %= source_h;
    origin -= wrap * source_h;
    let split = i64::from(image_x + slice_width >= source_w);
    let source_rows = source_h >> 5;
    let shift = 0x200i64 >> bg.image_siz;
    let mask = shift - 1;
    let (capacity, max_slice) = if bg.image_siz == 3 {
        (480, 0x2800)
    } else {
        (
            if bg.image_fmt == 2 { 256 } else { 512 },
            (((i64::from(bg.frame_w) * scale_w) >> 7) + bilerp * 32).min(source_w),
        )
    };
    let line = (max_slice + mask) / shift + 1;
    let image_rows = capacity / line - bilerp;
    let frame_rows = (image_rows << 20) / scale_h;
    let stride = (source_w / shift) * 8;
    if source_rows == 0 || image_rows <= 0 || frame_rows <= 0 || stride == 0 {
        invalid_parameters(cx, c.opcode());
        return Err(());
    }
    let mut image_offset = (image_y - origin) << 5;
    if image_offset < 0 {
        image_offset -= scale_h - 1;
    }
    let frame_offset = (image_offset / scale_h) << 10;
    let slice = frame_offset.div_euclid(frame_rows);
    let slice_start = frame_rows * slice;
    let hidden = frame_offset - (slice_start & !0x3ff);
    let image_hidden = (hidden >> 10) * scale_h;
    let mut count = (slice_start & 0x3ff) + frame_rows - hidden;
    let mut t = (image_hidden >> 5) & 0x1f;
    let mut slice_rows = image_rows - (image_hidden >> 10);
    let image_row =
        (image_rows * slice + (origin & !0x1f) / 32 + (image_hidden >> 10)).rem_euclid(source_rows);
    let image_top = cx.mem.resolve(bg.image_ptr).map_err(|error| {
        cx.diags.push(Diagnostic {
            at: cx.pc,
            kind: DiagKind::MemoryRead {
                access: crate::MemoryAccess::Texture,
                error,
            },
        })
    })?;
    let x_offset = (image_x / shift) * 8;
    let mut pointer = source_address(cx, image_top, stride * image_row + x_offset)?;
    let mut s = image_x & mask;
    if flip {
        s = -(s + image_width);
    }
    set_tile_typed(
        7,
        TileDescriptor {
            siz: if bg.image_siz == 3 { 3 } else { 2 },
            line: line as u16,
            ..Default::default()
        },
        cx,
    );
    set_tile_typed(
        0,
        TileDescriptor {
            fmt: bg.image_fmt,
            siz: bg.image_siz,
            line: line as u16,
            palette: bg.image_pal as u8,
            cms: 1,
            cmt: 1,
            masks: 15,
            maskt: 15,
            ..Default::default()
        },
        cx,
    );
    set_tile_size_typed(0, [0; 4], cx);
    let mut remain = source_rows - image_row;
    while frame_remain > 0 {
        let mut draw = count >> 10;
        if draw <= 0 {
            remain -= slice_rows;
            if remain > 0 {
                pointer = source_address(cx, pointer, stride * slice_rows)?;
            } else {
                pointer = source_address(cx, image_top, -remain * stride + x_offset)?;
                remain += source_rows;
            }
        } else {
            count &= 0x3ff;
            frame_remain -= draw;
            if frame_remain < 0 {
                draw += frame_remain;
                slice_rows += ((frame_remain * scale_h) >> 10) + 1;
                slice_rows = slice_rows.min(image_rows);
            }
            load_1cyc(
                cx,
                &bg,
                image_top,
                &mut pointer,
                &mut remain,
                stride,
                x_offset,
                source_rows,
                line,
                slice_rows,
                bilerp,
                split,
            )?;
            let end_y = frame_y + draw;
            let emission = emit_texrect(
                TexRect {
                    rect: crate::scene::TexRectBounds {
                        ulx: x as i32,
                        uly: (frame_y * 4) as i32,
                        lrx: x_end as i32,
                        lry: (end_y * 4) as i32,
                    },
                    tile: 0,
                    uls: s as i16,
                    ult: t as i16,
                    dsdx: scale_w as i16,
                    dtdy: scale_h as i16,
                    flip: false,
                },
                cx,
            )
            .ok_or(())?;
            emissions.push(emission);
            frame_y = end_y;
        }
        count += frame_rows;
        slice_rows = image_rows;
        t = 0;
    }
    Ok(emissions)
}

pub(crate) fn bg_copy<M: Rdram>(c: &Cmd, cx: &mut Ctx<M>) -> Result<Vec<Emission<'static>>, ()> {
    let bg = read_bg(c, cx, ObjBgKind::Copy)?;
    let product = (u32::from(bg.tmem_size) << 16) | u32::from(bg.tmem_load_sh);
    let rows = i64::from(bg.tmem_h) / 4;
    let image_width = i64::from(bg.image_w) / 4;
    let image_height = i64::from(bg.image_h) / 4;
    if bg.image_load != 0xfff4
        || rows == 0
        || image_width == 0
        || image_height == 0
        || bg.tmem_size_w == 0
        || product != u32::from(bg.tmem_h) * u32::from(bg.tmem_size_w)
        || bg.image_flip & !1 != 0
    {
        invalid_parameters(cx, c.opcode());
        return Err(());
    }
    let frame_x = i64::from(bg.frame_x);
    let frame_y = i64::from(bg.frame_y);
    let scissor = cx.rdp.scissor;
    let left = frame_x.max(i64::from(scissor.ulx) * 4);
    let top = frame_y.max(i64::from(scissor.uly) * 4);
    let right = (frame_x + i64::from(bg.frame_w)).min(i64::from(scissor.lrx) * 4);
    let bottom = (frame_y + i64::from(bg.frame_h)).min(i64::from(scissor.lry) * 4);
    let width = (right - left) / 4;
    let height = (bottom - top) / 4;
    let mut emissions = Vec::new();
    if width <= 0 || height <= 0 {
        return Ok(emissions);
    }
    let image_top = cx.mem.resolve(bg.image_ptr).map_err(|error| {
        cx.diags.push(Diagnostic {
            at: cx.pc,
            kind: DiagKind::MemoryRead {
                access: crate::MemoryAccess::Texture,
                error,
            },
        })
    })?;
    cx.rsp.set_texture_image(
        bg.image_fmt,
        bg.image_siz,
        (image_width - 1) as u16,
        image_top,
        cx.rdp,
    );
    let flip = bg.image_flip & 1 != 0;
    let source_x = i64::from(bg.image_x) / 32;
    let source_y = i64::from(bg.image_y) / 32 + (top - frame_y) / 4;
    let x_skip = (left - frame_x) / 4;
    let capacity = if bg.image_fmt == 2 || bg.image_siz == 3 {
        256
    } else {
        512
    };
    let texels_per_word = 16 >> bg.image_siz;
    let mut y = 0;
    while y < height {
        let sy = (source_y + y).rem_euclid(image_height);
        let strip_height = rows.min(height - y).min(image_height - sy);
        let mut x = 0;
        while x < width {
            let sx = if flip {
                source_x + i64::from(bg.frame_w) / 4 - 1 - x_skip - x
            } else {
                source_x + x_skip + x
            };
            let sx = sx.rem_euclid(image_width);
            let strip_width = (width - x).min(if flip { sx + 1 } else { image_width - sx });
            let source_left = if flip { sx - strip_width + 1 } else { sx };
            let aligned_left = source_left / texels_per_word * texels_per_word;
            let offset = source_left - aligned_left;
            let load_width = offset + strip_width;
            let bits = load_width * (1 << bg.image_siz);
            let divisor = if bg.image_siz == 3 { 32 } else { 16 };
            let line = (bits + divisor - 1) / divisor;
            if line * strip_height > capacity {
                invalid_parameters(cx, c.opcode());
                return Err(());
            }
            let tile = TileDescriptor {
                fmt: bg.image_fmt,
                siz: bg.image_siz,
                line: line as u16,
                palette: bg.image_pal as u8,
                ..Default::default()
            };
            set_tile_typed(7, tile.clone(), cx);
            set_tile_typed(
                0,
                TileDescriptor {
                    cms: u8::from(flip),
                    masks: if flip { 15 } else { 0 },
                    ..tile
                },
                cx,
            );
            set_tile_size_typed(
                0,
                [
                    0,
                    0,
                    ((load_width - 1) * 4) as u16,
                    ((strip_height - 1) * 4) as u16,
                ],
                cx,
            );
            load_tile_typed(
                7,
                [
                    (aligned_left * 4) as u16,
                    (sy * 4) as u16,
                    ((aligned_left + load_width - 1) * 4) as u16,
                    ((sy + strip_height - 1) * 4) as u16,
                ],
                cx,
            )?;
            // COPY already divides dsdx by four and extends raw inclusive endpoints by one pixel.
            let emission = emit_texrect(
                TexRect {
                    rect: crate::scene::TexRectBounds {
                        ulx: (left + x * 4) as i32,
                        uly: (top + y * 4) as i32,
                        lrx: (left + (x + strip_width - 1) * 4) as i32,
                        lry: (top + (y + strip_height - 1) * 4) as i32,
                    },
                    tile: 0,
                    uls: (if flip { -load_width } else { offset } * 32) as i16,
                    ult: 0,
                    dsdx: 0x1000,
                    dtdy: 0x0400,
                    flip: false,
                },
                cx,
            )
            .ok_or(())?;
            emissions.push(emission);
            x += strip_width;
        }
        y += strip_height;
    }
    Ok(emissions)
}
