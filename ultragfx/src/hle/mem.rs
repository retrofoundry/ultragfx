//! Fallible memory readers for N64 byte images and consumer-provided address spaces.

use std::borrow::Cow;
use std::cell::Cell;
use std::ops::Range;

/// A decoded 4×4 matrix in the renderer's row-vector convention.
pub type Matrix = [[f32; 4]; 4];

/// Binary layout of matrices and vertices referenced by display-list commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GbiDataFormat {
    /// Big-endian s15.16 split matrices and 16-byte fixed-point vertices in an image backend.
    Fixed,
    /// Native `f32` matrices and 24-byte `GBI_FLOATS` vertices in a host backend.
    Float,
}

/// Segmented addresses are `(segment << 24) | offset`, so they always sit below this bound. Values
/// at or above it are already resolved and pass through untouched, even when bits 24..28 happen to
/// name a segment that is set. Host reads below it fail, so native inputs must sit at or above it.
#[cfg(any(
    feature = "capture",
    all(not(target_arch = "wasm32"), target_pointer_width = "64")
))]
pub(crate) const SEGMENT_SPACE_END: u64 = 0x1000_0000;

/// Why a memory operation could not produce its complete requested value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemoryErrorKind {
    OutOfBounds,
    AddressOverflow,
    UnsupportedFormat,
    Unavailable,
}

/// A failed memory operation and the exact byte span it requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryError {
    pub address: u64,
    pub length: u64,
    pub kind: MemoryErrorKind,
}

fn memory_error(address: u64, length: u64, kind: MemoryErrorKind) -> MemoryError {
    MemoryError {
        address,
        length,
        kind,
    }
}

/// The background union variant consumed by a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjBgKind {
    Copy,
    Scaled,
}

/// A decoded S2DEX background; fields outside the requested variant are zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjBg {
    pub image_x: u16,
    pub image_w: u16,
    pub frame_x: i16,
    pub frame_w: u16,
    pub image_y: u16,
    pub image_h: u16,
    pub frame_y: i16,
    pub frame_h: u16,
    pub image_ptr: u64,
    pub image_load: u16,
    pub image_fmt: u8,
    pub image_siz: u8,
    pub image_pal: u16,
    pub image_flip: u16,
    pub tmem_w: u16,
    pub tmem_h: u16,
    pub tmem_load_sh: u16,
    pub tmem_load_th: u16,
    pub tmem_size_w: u16,
    pub tmem_size: u16,
    pub scale_w: u16,
    pub scale_h: u16,
    pub image_y_orig: i32,
}

/// A decoded S2DEX `uObjTxtr`. The halfwords at 8, 10 and 12 are `tmem`, `tsize` and `tline`
/// for a block load, `tmem`, `twidth` and `theight` for a tile load, and `phead`, `pnum` and
/// `zero` for a TLUT load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjTxtr {
    pub load_type: u32,
    pub image: u64,
    pub tmem: u16,
    pub tsize: u16,
    pub tline: u16,
    pub sid: u16,
    pub flag: u32,
    pub mask: u32,
}

/// A decoded S2DEX `uObjSprite`, without its padding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjSprite {
    pub obj_x: i16,
    pub scale_w: u16,
    pub image_w: u16,
    pub obj_y: i16,
    pub scale_h: u16,
    pub image_h: u16,
    pub image_stride: u16,
    pub image_adrs: u16,
    pub image_fmt: u8,
    pub image_siz: u8,
    pub image_pal: u8,
    pub image_flags: u8,
}

/// The 2D matrix struct consumed by a `G_OBJ_MOVEMEM` command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjMtxKind {
    /// `uObjMtx`, 24 bytes.
    Full,
    /// `uObjSubMtx`, 8 bytes: `x`, `y`, `base_scale_x` and `base_scale_y` at offset 0.
    Sub,
}

/// A decoded S2DEX 2D matrix; `a` to `d` are zero for `ObjMtxKind::Sub`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjMtx {
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub d: i32,
    pub x: i16,
    pub y: i16,
    pub base_scale_x: u16,
    pub base_scale_y: u16,
}

fn struct_fields(
    address: u64,
    mut word: impl FnMut(u64, usize) -> Result<u64, MemoryError>,
) -> impl FnMut(u64, usize) -> Result<u64, MemoryError> {
    move |offset, length| {
        let field = address
            .checked_add(offset)
            .ok_or_else(|| memory_error(address, offset, MemoryErrorKind::AddressOverflow))?;
        word(field, length)
    }
}

pub(crate) fn decode_obj_txtr(
    address: u64,
    word: impl FnMut(u64, usize) -> Result<u64, MemoryError>,
) -> Result<ObjTxtr, MemoryError> {
    let mut read = struct_fields(address, word);
    Ok(ObjTxtr {
        load_type: read(0, 4)? as u32,
        image: read(4, 4)?,
        tmem: read(8, 2)? as u16,
        tsize: read(10, 2)? as u16,
        tline: read(12, 2)? as u16,
        sid: read(14, 2)? as u16,
        flag: read(16, 4)? as u32,
        mask: read(20, 4)? as u32,
    })
}

pub(crate) fn decode_obj_sprite(
    address: u64,
    word: impl FnMut(u64, usize) -> Result<u64, MemoryError>,
) -> Result<ObjSprite, MemoryError> {
    let mut read = struct_fields(address, word);
    Ok(ObjSprite {
        obj_x: read(0, 2)? as i16,
        scale_w: read(2, 2)? as u16,
        image_w: read(4, 2)? as u16,
        obj_y: read(8, 2)? as i16,
        scale_h: read(10, 2)? as u16,
        image_h: read(12, 2)? as u16,
        image_stride: read(16, 2)? as u16,
        image_adrs: read(18, 2)? as u16,
        image_fmt: read(20, 1)? as u8,
        image_siz: read(21, 1)? as u8,
        image_pal: read(22, 1)? as u8,
        image_flags: read(23, 1)? as u8,
    })
}

pub(crate) fn decode_obj_mtx(
    address: u64,
    kind: ObjMtxKind,
    word: impl FnMut(u64, usize) -> Result<u64, MemoryError>,
) -> Result<ObjMtx, MemoryError> {
    let mut read = struct_fields(address, word);
    let mut mtx = ObjMtx::default();
    let tail = match kind {
        ObjMtxKind::Full => {
            mtx.a = read(0, 4)? as i32;
            mtx.b = read(4, 4)? as i32;
            mtx.c = read(8, 4)? as i32;
            mtx.d = read(12, 4)? as i32;
            16
        }
        ObjMtxKind::Sub => 0,
    };
    mtx.x = read(tail, 2)? as i16;
    mtx.y = read(tail + 2, 2)? as i16;
    mtx.base_scale_x = read(tail + 4, 2)? as u16;
    mtx.base_scale_y = read(tail + 6, 2)? as u16;
    Ok(mtx)
}

pub(crate) fn decode_obj_bg(
    address: u64,
    kind: ObjBgKind,
    pointer_bytes: usize,
    word: impl FnMut(u64, usize) -> Result<u64, MemoryError>,
) -> Result<ObjBg, MemoryError> {
    let mut read = struct_fields(address, word);
    let tail = 16 + pointer_bytes as u64;
    let mut bg = ObjBg {
        image_x: read(0, 2)? as u16,
        image_w: read(2, 2)? as u16,
        frame_x: read(4, 2)? as i16,
        frame_w: read(6, 2)? as u16,
        image_y: read(8, 2)? as u16,
        image_h: read(10, 2)? as u16,
        frame_y: read(12, 2)? as i16,
        frame_h: read(14, 2)? as u16,
        image_ptr: read(16, pointer_bytes)?,
        image_load: read(tail, 2)? as u16,
        image_fmt: read(tail + 2, 1)? as u8,
        image_siz: read(tail + 3, 1)? as u8,
        image_pal: read(tail + 4, 2)? as u16,
        image_flip: read(tail + 6, 2)? as u16,
        ..ObjBg::default()
    };
    match kind {
        ObjBgKind::Copy => {
            bg.tmem_w = read(tail + 8, 2)? as u16;
            bg.tmem_h = read(tail + 10, 2)? as u16;
            bg.tmem_load_sh = read(tail + 12, 2)? as u16;
            bg.tmem_load_th = read(tail + 14, 2)? as u16;
            bg.tmem_size_w = read(tail + 16, 2)? as u16;
            bg.tmem_size = read(tail + 18, 2)? as u16;
        }
        ObjBgKind::Scaled => {
            bg.scale_w = read(tail + 8, 2)? as u16;
            bg.scale_h = read(tail + 10, 2)? as u16;
            bg.image_y_orig = read(tail + 12, 4)? as i32;
        }
    }
    Ok(bg)
}

/// One memory address space for the interpreter.
///
/// Reads return decoded values rather than guest-layout structs. Implementations must return an
/// error unless the complete requested value is available. Image backends may zero-fill data
/// beyond installed RDRAM and report it through `take_zero_filled_read`. Command fetches must
/// remain strict. `in_bounds` is advisory: a successful query does not make a later read infallible.
/// The interpreter does not catch panics from a consumer implementation.
pub trait Rdram {
    /// Describes the byte and address layout used when recording memory reads.
    #[cfg(feature = "capture")]
    fn capture_layout(&self) -> Option<crate::capture::SourceLayout> {
        None
    }

    /// Takes the first data span zero-filled since the previous call. Backends that refuse
    /// out-of-range data reads leave this at its default. The interpreter reports one warning
    /// per task; wrappers must forward this to preserve their backend's diagnostics.
    fn take_zero_filled_read(&self) -> Option<(u64, u64)> {
        None
    }

    /// Stores the raw segment value selected by the low four bits of `segment`.
    fn set_segment(&mut self, segment: u32, value: u64);

    /// Resolves an image operand without the display-list alignment mask.
    fn resolve(&self, address: u64) -> Result<u64, MemoryError>;

    /// Resolves an address operand using the backend's masked-address convention.
    ///
    /// `RdramImage` applies `& 0x00ff_fff8` after 32-bit segmented resolution. Native host
    /// backends preserve the full pointer because masking it would change its identity.
    fn resolve_masked(&self, address: u64) -> Result<u64, MemoryError>;

    fn read_command(&self, address: u64) -> Result<Command, MemoryError>;
    fn command_stride(&self) -> u64;
    /// An advisory range query that returns false if `address + length` overflows.
    fn in_bounds(&self, address: u64, length: u64) -> bool;
    /// Validates a write-back span without reading it; backends are permissive by default.
    fn check_write(&self, _address: u64, _length: u64) -> Result<(), MemoryError> {
        Ok(())
    }
    fn read_u8(&self, address: u64) -> Result<u8, MemoryError>;
    fn read_i8(&self, address: u64) -> Result<i8, MemoryError>;
    fn read_i16(&self, address: u64) -> Result<i16, MemoryError>;
    fn read_u16(&self, address: u64) -> Result<u16, MemoryError>;
    /// Returns exactly `length` bytes or an error.
    fn read_bytes(&self, address: u64, length: usize) -> Result<Cow<'_, [u8]>, MemoryError>;
    fn read_matrix(&self, address: u64, format: GbiDataFormat) -> Result<Matrix, MemoryError>;

    /// Returns a Fixed-format matrix as its 16 s15.16 words, row-major, which F3DZEX's W branch
    /// needs to compute W as the RSP does. Backends without them leave this at its default and
    /// the branch uses the float matrix; wrappers must forward this to preserve their backend's
    /// words.
    fn read_matrix_words(&self, _address: u64) -> Result<Option<[i32; 16]>, MemoryError> {
        Ok(None)
    }

    /// Decodes the fields consumed by the requested background command.
    fn read_obj_bg(&self, address: u64, _kind: ObjBgKind) -> Result<ObjBg, MemoryError> {
        Err(memory_error(address, 0, MemoryErrorKind::UnsupportedFormat))
    }

    /// Decodes an S2DEX `uObjTxtr`. Backends without the N64 layout leave this at its default;
    /// wrappers must forward this and the other object reads to keep object commands.
    fn read_obj_txtr(&self, address: u64) -> Result<ObjTxtr, MemoryError> {
        Err(memory_error(address, 0, MemoryErrorKind::UnsupportedFormat))
    }

    /// Decodes an S2DEX `uObjSprite`.
    fn read_obj_sprite(&self, address: u64) -> Result<ObjSprite, MemoryError> {
        Err(memory_error(address, 0, MemoryErrorKind::UnsupportedFormat))
    }

    /// Decodes the requested S2DEX 2D matrix struct.
    fn read_obj_mtx(&self, address: u64, _kind: ObjMtxKind) -> Result<ObjMtx, MemoryError> {
        Err(memory_error(address, 0, MemoryErrorKind::UnsupportedFormat))
    }

    /// Returns the byte stride for the requested vertex layout.
    ///
    /// The default image layout supports only 16-byte Fixed vertices.
    fn vertex_stride(&self, format: GbiDataFormat) -> Result<u64, MemoryError> {
        match format {
            GbiDataFormat::Fixed => Ok(16),
            GbiDataFormat::Float => Err(memory_error(0, 0, MemoryErrorKind::UnsupportedFormat)),
        }
    }

    /// Decodes one Fixed vertex using this backend's scalar readers.
    ///
    /// The default layout is `s16 pos[3]@0`, `u16 flag@6`, `s16 st[2]@8`, and
    /// `u8 rgba[4]@12`, with a 16-byte stride. Float requires a backend override.
    fn read_vertex(&self, address: u64, format: GbiDataFormat) -> Result<RawVertex, MemoryError> {
        if format != GbiDataFormat::Fixed {
            return Err(memory_error(
                address,
                24,
                MemoryErrorKind::UnsupportedFormat,
            ));
        }
        read_fixed_vertex(self, address)
    }

    /// Allows VI addresses to be interpreted as physical offsets in this memory image.
    ///
    /// This does not authorize memory reads during presentation.
    fn is_rdram_image(&self) -> bool {
        false
    }
}

fn read_fixed_vertex(
    memory: &(impl Rdram + ?Sized),
    address: u64,
) -> Result<RawVertex, MemoryError> {
    address
        .checked_add(16)
        .ok_or_else(|| memory_error(address, 16, MemoryErrorKind::AddressOverflow))?;
    Ok(RawVertex {
        pos: [
            memory.read_i16(address)? as f32,
            memory.read_i16(address + 2)? as f32,
            memory.read_i16(address + 4)? as f32,
        ],
        st: [
            memory.read_i16(address + 8)?,
            memory.read_i16(address + 10)?,
        ],
        rgba: [
            memory.read_u8(address + 12)?,
            memory.read_u8(address + 13)?,
            memory.read_u8(address + 14)?,
            memory.read_u8(address + 15)?,
        ],
    })
}

/// A decoded vertex. This is not a guest-memory layout and must not be used for casting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawVertex {
    pub pos: [f32; 3],
    pub st: [i16; 2],
    pub rgba: [u8; 4],
}

/// A decoded display-list command. This is not a guest-memory layout and must not be cast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub w0: u32,
    pub w1: u32,
    pub w1_addr: u64,
}

/// A safe, contiguous big-endian RDRAM image.
pub struct RdramImage<'a> {
    pub bytes: &'a [u8],
    /// Raw 32-bit segment bases. Zero bases preserve the operand's physical low 24 bits.
    pub segments: [u32; 16],
    zero_filled_read: Cell<Option<(u64, u64)>>,
}

impl<'a> RdramImage<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            segments: [0; 16],
            zero_filled_read: Cell::new(None),
        }
    }

    /// Stores the low 32 bits of a raw segment value without resolving or masking it.
    pub fn set_segment(&mut self, segment: u32, value: u64) {
        self.segments[(segment & 0x0f) as usize] = value as u32;
    }

    /// Performs 32-bit segmented resolution without the display-list alignment mask.
    ///
    /// The segment base plus the low 24-bit offset intentionally wraps as a `u32`, matching N64
    /// address arithmetic. Inputs outside the 32-bit image address space are rejected.
    pub fn from_segmented(&self, address: u64) -> Result<u64, MemoryError> {
        let address = u32::try_from(address)
            .map_err(|_| memory_error(address, 0, MemoryErrorKind::AddressOverflow))?;
        let segment = ((address >> 24) & 0x0f) as usize;
        Ok(self.segments[segment].wrapping_add(address & 0x00ff_ffff) as u64)
    }

    /// Resolves a 32-bit segmented address, then applies `& 0x00ff_fff8`.
    pub fn from_segmented_masked(&self, address: u64) -> Result<u64, MemoryError> {
        Ok(self.from_segmented(address)? & 0x00ff_fff8)
    }

    pub fn read_i16(&self, address: u64) -> Result<i16, MemoryError> {
        let bytes = self.read_data(address, 2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_u16(&self, address: u64) -> Result<u16, MemoryError> {
        let bytes = self.read_data(address, 2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_u8(&self, address: u64) -> Result<u8, MemoryError> {
        Ok(self.read_data(address, 1)?[0])
    }

    pub fn read_i8(&self, address: u64) -> Result<i8, MemoryError> {
        Ok(self.read_u8(address)? as i8)
    }

    pub fn read_slice(&self, address: u64, length: usize) -> Result<&[u8], MemoryError> {
        let range = self.range(address, length as u64)?;
        Ok(&self.bytes[range])
    }

    fn data_word(&self, address: u64, length: usize) -> Result<u64, MemoryError> {
        Ok(self
            .read_data(address, length)?
            .iter()
            .fold(0, |v, &b| (v << 8) | b as u64))
    }

    fn read_data(&self, address: u64, length: usize) -> Result<Cow<'_, [u8]>, MemoryError> {
        const ADDRESS_SPACE: usize = 0x0100_0000;
        address.checked_add(length as u64).ok_or_else(|| {
            memory_error(address, length as u64, MemoryErrorKind::AddressOverflow)
        })?;
        let start = u32::try_from(address)
            .map_err(|_| memory_error(address, length as u64, MemoryErrorKind::AddressOverflow))?
            as usize
            & (ADDRESS_SPACE - 1);
        if length <= ADDRESS_SPACE - start {
            if let Some(bytes) = self.bytes.get(start..start + length) {
                return Ok(Cow::Borrowed(bytes));
            }
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| memory_error(address, length as u64, MemoryErrorKind::Unavailable))?;
        bytes.resize(length, 0);
        let mut remaining = bytes.as_mut_slice();
        let mut physical = start;
        let mut zero_filled = false;
        while !remaining.is_empty() {
            let count = remaining.len().min(ADDRESS_SPACE - physical);
            let (chunk, rest) = remaining.split_at_mut(count);
            let available = self.bytes.len().saturating_sub(physical).min(count);
            if available != 0 {
                chunk[..available].copy_from_slice(&self.bytes[physical..physical + available]);
            }
            zero_filled |= available < count;
            remaining = rest;
            physical = 0;
        }
        if zero_filled && self.zero_filled_read.get().is_none() {
            self.zero_filled_read
                .set(Some((start as u64, length as u64)));
        }
        Ok(Cow::Owned(bytes))
    }

    fn decode_matrix_words(&self, address: u64) -> Result<[i32; 16], MemoryError> {
        let bytes = self.read_data(address, 64)?;
        Ok(std::array::from_fn(|k| {
            let integer = i16::from_be_bytes([bytes[k * 2], bytes[k * 2 + 1]]) as i32;
            let fraction = u16::from_be_bytes([bytes[32 + k * 2], bytes[33 + k * 2]]) as i32;
            (integer << 16) | fraction
        }))
    }

    fn decode_matrix(&self, address: u64) -> Result<Matrix, MemoryError> {
        let words = self.decode_matrix_words(address)?;
        Ok(std::array::from_fn(|row| {
            std::array::from_fn(|column| words[row * 4 + column] as f32 / 65536.0)
        }))
    }

    fn range(&self, address: u64, length: u64) -> Result<Range<usize>, MemoryError> {
        let end = address
            .checked_add(length)
            .ok_or_else(|| memory_error(address, length, MemoryErrorKind::AddressOverflow))?;
        if end > self.bytes.len() as u64 {
            return Err(memory_error(address, length, MemoryErrorKind::OutOfBounds));
        }
        let start = usize::try_from(address)
            .map_err(|_| memory_error(address, length, MemoryErrorKind::OutOfBounds))?;
        let end = usize::try_from(end)
            .map_err(|_| memory_error(address, length, MemoryErrorKind::OutOfBounds))?;
        Ok(start..end)
    }
}

impl Rdram for RdramImage<'_> {
    fn take_zero_filled_read(&self) -> Option<(u64, u64)> {
        self.zero_filled_read.take()
    }

    #[cfg(feature = "capture")]
    fn capture_layout(&self) -> Option<crate::capture::SourceLayout> {
        Some(crate::capture::SourceLayout {
            memory: crate::capture::MemoryLayout::IMAGE,
            segments: self.segments.map(u64::from),
        })
    }

    fn set_segment(&mut self, segment: u32, value: u64) {
        RdramImage::set_segment(self, segment, value);
    }

    fn resolve(&self, address: u64) -> Result<u64, MemoryError> {
        self.from_segmented(address)
    }

    fn resolve_masked(&self, address: u64) -> Result<u64, MemoryError> {
        self.from_segmented_masked(address)
    }

    fn read_obj_bg(&self, address: u64, kind: ObjBgKind) -> Result<ObjBg, MemoryError> {
        decode_obj_bg(address, kind, 4, |address, length| {
            self.data_word(address, length)
        })
    }

    fn read_obj_txtr(&self, address: u64) -> Result<ObjTxtr, MemoryError> {
        decode_obj_txtr(address, |address, length| self.data_word(address, length))
    }

    fn read_obj_sprite(&self, address: u64) -> Result<ObjSprite, MemoryError> {
        decode_obj_sprite(address, |address, length| self.data_word(address, length))
    }

    fn read_obj_mtx(&self, address: u64, kind: ObjMtxKind) -> Result<ObjMtx, MemoryError> {
        decode_obj_mtx(address, kind, |address, length| {
            self.data_word(address, length)
        })
    }

    fn read_command(&self, address: u64) -> Result<Command, MemoryError> {
        let bytes = self.read_slice(address, 8)?;
        let w0 = u32::from_be_bytes(bytes[..4].try_into().unwrap());
        let w1 = u32::from_be_bytes(bytes[4..].try_into().unwrap());
        Ok(Command {
            w0,
            w1,
            w1_addr: u64::from(w1),
        })
    }

    fn command_stride(&self) -> u64 {
        8
    }

    fn in_bounds(&self, address: u64, length: u64) -> bool {
        address
            .checked_add(length)
            .is_some_and(|end| end <= self.bytes.len() as u64)
    }

    fn read_u8(&self, address: u64) -> Result<u8, MemoryError> {
        RdramImage::read_u8(self, address)
    }

    fn read_i8(&self, address: u64) -> Result<i8, MemoryError> {
        RdramImage::read_i8(self, address)
    }

    fn read_i16(&self, address: u64) -> Result<i16, MemoryError> {
        RdramImage::read_i16(self, address)
    }

    fn read_u16(&self, address: u64) -> Result<u16, MemoryError> {
        RdramImage::read_u16(self, address)
    }

    fn read_bytes(&self, address: u64, length: usize) -> Result<Cow<'_, [u8]>, MemoryError> {
        self.read_data(address, length)
    }

    fn read_matrix(&self, address: u64, format: GbiDataFormat) -> Result<Matrix, MemoryError> {
        match format {
            GbiDataFormat::Fixed => self.decode_matrix(address),
            GbiDataFormat::Float => Err(memory_error(
                address,
                64,
                MemoryErrorKind::UnsupportedFormat,
            )),
        }
    }

    fn read_matrix_words(&self, address: u64) -> Result<Option<[i32; 16]>, MemoryError> {
        self.decode_matrix_words(address).map(Some)
    }

    fn read_vertex(&self, address: u64, format: GbiDataFormat) -> Result<RawVertex, MemoryError> {
        if format != GbiDataFormat::Fixed {
            return Err(memory_error(
                address,
                24,
                MemoryErrorKind::UnsupportedFormat,
            ));
        }
        let bytes = self.read_data(address, 16)?;
        read_fixed_vertex(&RdramImage::new(&bytes), 0)
    }

    fn is_rdram_image(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_reads_are_big_endian_and_exact() {
        let bytes = [0x12, 0x34, 0x80, 0xff, 0, 0, 0, 0];
        let image = RdramImage::new(&bytes);

        assert_eq!(image.read_u16(0), Ok(0x1234));
        assert_eq!(image.read_i8(2), Ok(-128));
        assert_eq!(image.read_slice(6, 2), Ok(&bytes[6..8]));
        assert_eq!(
            image.read_slice(7, 2),
            Err(memory_error(7, 2, MemoryErrorKind::OutOfBounds))
        );
    }

    #[test]
    fn image_range_addition_cannot_wrap() {
        let image = RdramImage::new(&[]);
        assert_eq!(
            image.read_slice(u64::MAX, 2),
            Err(memory_error(u64::MAX, 2, MemoryErrorKind::AddressOverflow,))
        );
        assert!(!image.in_bounds(u64::MAX, 2));
    }

    #[test]
    fn obj_structs_decode_each_field_at_its_offset() {
        let mut bytes = vec![0u8; 72];
        bytes[..24].copy_from_slice(&[
            0x00, 0xfc, 0x10, 0x34, 0x05, 0x01, 0x23, 0x40, 0x01, 0x00, 0x00, 0x1f, 0x00, 0x7f,
            0x00, 0x0c, 0x80, 0x00, 0x00, 0x01, 0xff, 0xff, 0xff, 0xfe,
        ]);
        bytes[24..48].copy_from_slice(&[
            0xff, 0x60, 0x04, 0x00, 0x03, 0xe8, 0xaa, 0xaa, 0x00, 0x40, 0x05, 0x55, 0x02, 0x00,
            0xbb, 0xbb, 0x00, 0x08, 0x01, 0x00, 0x02, 0x00, 0x01, 0x11,
        ]);
        bytes[48..72].copy_from_slice(&[
            0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x7f, 0xff, 0xff, 0xff, 0x00, 0x00,
            0xdd, 0xb4, 0xff, 0xf0, 0x00, 0x20, 0x04, 0x00, 0x02, 0x00,
        ]);
        let image = RdramImage::new(&bytes);
        assert_eq!(
            image.read_obj_txtr(0),
            Ok(ObjTxtr {
                load_type: 0x00fc_1034,
                image: 0x0501_2340,
                tmem: 256,
                tsize: 31,
                tline: 127,
                sid: 12,
                flag: 0x8000_0001,
                mask: 0xffff_fffe,
            })
        );
        assert_eq!(
            image.read_obj_sprite(24),
            Ok(ObjSprite {
                obj_x: -160,
                scale_w: 0x0400,
                image_w: 1000,
                obj_y: 64,
                scale_h: 0x0555,
                image_h: 512,
                image_stride: 8,
                image_adrs: 256,
                image_fmt: 2,
                image_siz: 0,
                image_pal: 1,
                image_flags: 0x11,
            })
        );
        let sub = ObjMtx {
            x: -16,
            y: 32,
            base_scale_x: 0x0400,
            base_scale_y: 0x0200,
            ..ObjMtx::default()
        };
        assert_eq!(
            image.read_obj_mtx(48, ObjMtxKind::Full),
            Ok(ObjMtx {
                a: -0x10000,
                b: 0x8000,
                c: i32::MAX,
                d: 0xddb4,
                ..sub
            })
        );
        assert_eq!(image.read_obj_mtx(64, ObjMtxKind::Sub), Ok(sub));
    }
}
