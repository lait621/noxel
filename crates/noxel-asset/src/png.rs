//! A complete pure-Rust PNG codec: inflate, deflate and the PNG container.
//!
//! Nothing here reaches for a C library or a crate: the DEFLATE decompressor,
//! the fixed-Huffman compressor, the CRC-32 and the Adler-32 are all
//! implemented in this file, in safe Rust.
//!
//! ## Decoding
//!
//! [`decode`] accepts everything the PNG specification allows for non-interlaced
//! images:
//!
//! * colour types 0 (grayscale), 2 (RGB), 3 (indexed + `PLTE`), 4 (gray+alpha)
//!   and 6 (RGBA);
//! * bit depths 1, 2, 4, 8 and 16;
//! * all five scanline filters (None, Sub, Up, Average, Paeth);
//! * `tRNS` transparency for colour types 0, 2 and 3;
//! * any number of `IDAT` chunks, and unknown ancillary chunks are skipped.
//!
//! Every chunk's CRC-32 is verified. Adam7-interlaced images are **refused**
//! with [`PngError::Unsupported`] rather than decoded into garbage.
//!
//! Decoding is bounded: [`MAX_DIMENSION`], [`MAX_DECODED_PIXELS`] and an exact
//! inflate output limit mean a decompression bomb is rejected quickly instead of
//! exhausting memory.
//!
//! ## Encoding
//!
//! [`encode`] always writes colour type 6 (RGBA), bit depth 8, filter 0 and no
//! interlace, using a fixed-Huffman DEFLATE stream with a greedy hash-chain
//! LZ77 matcher. `decode(encode(image)) == image` holds for every non-empty
//! image.
//!
//! ## Example
//!
//! ```
//! use noxel_asset::image::Image;
//! use noxel_asset::png;
//! use noxel_core::math::Color8;
//!
//! let image = Image::new(3, 2, Color8::rgb(10, 20, 30));
//! let bytes = png::encode(&image);
//! assert_eq!(png::decode(&bytes).unwrap(), image);
//! ```

use crate::image::Image;
use noxel_core::math::Color8;
use std::fmt;

/// The eight bytes every PNG file starts with.
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Largest width or height [`decode`] will accept.
pub const MAX_DIMENSION: u32 = 8192;

/// Largest total pixel count [`decode`] will accept (16 Mpx = 64 MiB of RGBA).
pub const MAX_DECODED_PIXELS: u64 = 16 * 1024 * 1024;

/// Hard ceiling on the number of bytes [`decode`] will inflate from one image.
pub const MAX_INFLATE_BYTES: usize = 256 * 1024 * 1024;

/// Anything that can go wrong while decoding a PNG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PngError {
    /// The eight-byte signature was missing or wrong.
    Signature,
    /// The file ended in the middle of a chunk or of the image data.
    Truncated {
        /// Byte offset at which the data ran out.
        at: usize,
    },
    /// A chunk's CRC-32 did not match its contents.
    Crc {
        /// The chunk type, e.g. `"IDAT"`.
        chunk: String,
        /// The CRC-32 computed over the chunk.
        expected: u32,
        /// The CRC-32 stored in the file.
        found: u32,
    },
    /// A valid PNG feature this codec deliberately does not implement.
    Unsupported(String),
    /// The stream is malformed.
    Invalid(String),
    /// A hard safety limit was exceeded.
    TooLarge(String),
    /// The zlib/DEFLATE stream could not be decoded.
    Inflate(String),
}

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signature => write!(f, "not a PNG file: bad signature"),
            Self::Truncated { at } => write!(f, "truncated PNG file at byte {at}"),
            Self::Crc {
                chunk,
                expected,
                found,
            } => write!(
                f,
                "CRC mismatch in {chunk} chunk: computed {expected:#010x}, file says {found:#010x}"
            ),
            Self::Unsupported(what) => write!(f, "unsupported PNG feature: {what}"),
            Self::Invalid(what) => write!(f, "invalid PNG: {what}"),
            Self::TooLarge(what) => write!(f, "PNG exceeds a safety limit: {what}"),
            Self::Inflate(what) => write!(f, "deflate error: {what}"),
        }
    }
}

impl std::error::Error for PngError {}

/// The five PNG scanline filter types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// No filtering; the raw bytes are stored.
    None,
    /// Subtracts the byte `bpp` positions to the left.
    Sub,
    /// Subtracts the byte directly above.
    Up,
    /// Subtracts the average of the left and above bytes.
    Average,
    /// Subtracts the Paeth predictor of the left, above and above-left bytes.
    Paeth,
}

impl Filter {
    /// The filter's on-disk byte value.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Sub => 1,
            Self::Up => 2,
            Self::Average => 3,
            Self::Paeth => 4,
        }
    }

    /// Parses a filter byte, rejecting values above 4.
    pub fn from_u8(value: u8) -> Result<Self, PngError> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Sub),
            2 => Ok(Self::Up),
            3 => Ok(Self::Average),
            4 => Ok(Self::Paeth),
            other => Err(PngError::Invalid(format!(
                "unknown scanline filter {other}"
            ))),
        }
    }
}

/// The Paeth predictor, as defined by the PNG specification.
#[must_use]
pub fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let pa = (p - i16::from(a)).abs();
    let pb = (p - i16::from(b)).abs();
    let pc = (p - i16::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Applies a PNG filter to one scanline, in place.
///
/// `previous` is the **unfiltered** previous scanline (empty for the first row)
/// and `current` is the unfiltered row on entry, the filtered row on exit.
/// `bpp` is the number of bytes per complete pixel, rounded up to at least 1.
pub fn filter_scanline(filter: Filter, bpp: usize, previous: &[u8], current: &mut [u8]) {
    let bpp = bpp.max(1);
    let original = current.to_vec();
    for i in 0..current.len() {
        let left = if i >= bpp { original[i - bpp] } else { 0 };
        let up = previous.get(i).copied().unwrap_or(0);
        let up_left = if i >= bpp {
            previous.get(i - bpp).copied().unwrap_or(0)
        } else {
            0
        };
        let predictor = match filter {
            Filter::None => 0,
            Filter::Sub => left,
            Filter::Up => up,
            Filter::Average => ((u16::from(left) + u16::from(up)) / 2) as u8,
            Filter::Paeth => paeth_predictor(left, up, up_left),
        };
        current[i] = original[i].wrapping_sub(predictor);
    }
}

/// Reverses [`filter_scanline`], in place.
///
/// `previous` is the already-reconstructed previous scanline (empty for the
/// first row).
pub fn unfilter_scanline(filter: Filter, bpp: usize, previous: &[u8], current: &mut [u8]) {
    let bpp = bpp.max(1);
    for i in 0..current.len() {
        let left = if i >= bpp { current[i - bpp] } else { 0 };
        let up = previous.get(i).copied().unwrap_or(0);
        let up_left = if i >= bpp {
            previous.get(i - bpp).copied().unwrap_or(0)
        } else {
            0
        };
        let predictor = match filter {
            Filter::None => 0,
            Filter::Sub => left,
            Filter::Up => up,
            Filter::Average => ((u16::from(left) + u16::from(up)) / 2) as u8,
            Filter::Paeth => paeth_predictor(left, up, up_left),
        };
        current[i] = current[i].wrapping_add(predictor);
    }
}

/// The decoded header fields this codec cares about.
#[derive(Clone, Copy, Debug)]
struct Ihdr {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
}

impl Ihdr {
    fn channels(self) -> usize {
        match self.color_type {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        }
    }

    fn bits_per_pixel(self) -> usize {
        self.channels() * self.bit_depth as usize
    }

    /// Bytes per complete pixel, rounded up, with a floor of one.
    fn bpp(self) -> usize {
        self.bits_per_pixel().div_ceil(8).max(1)
    }

    fn stride(self) -> usize {
        (self.width as usize * self.bits_per_pixel()).div_ceil(8)
    }

    /// Total uncompressed image-data size: every row plus its filter byte.
    fn raw_len(self) -> usize {
        (self.stride() + 1) * self.height as usize
    }
}

/// Decodes a PNG file into an RGBA image.
///
/// See the module documentation for the exact subset of the format that is
/// accepted. Every error path is a [`PngError`]; this function never panics and
/// never allocates more than the documented limits allow.
pub fn decode(bytes: &[u8]) -> Result<Image, PngError> {
    if bytes.len() < SIGNATURE.len() || bytes[..SIGNATURE.len()] != SIGNATURE {
        return Err(PngError::Signature);
    }

    let mut pos = SIGNATURE.len();
    let mut header: Option<Ihdr> = None;
    let mut palette: Option<Vec<Color8>> = None;
    let mut transparency: Option<Vec<u8>> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut saw_iend = false;

    while pos < bytes.len() {
        if pos + 8 > bytes.len() {
            return Err(PngError::Truncated { at: bytes.len() });
        }
        let length =
            u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]);
        let kind: [u8; 4] = [
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ];
        let data_start = pos + 8;
        let data_end = data_start
            .checked_add(length as usize)
            .ok_or(PngError::Truncated { at: bytes.len() })?;
        let crc_end = data_end
            .checked_add(4)
            .ok_or(PngError::Truncated { at: bytes.len() })?;
        if crc_end > bytes.len() {
            return Err(PngError::Truncated { at: bytes.len() });
        }
        let data = &bytes[data_start..data_end];
        let stored_crc = u32::from_be_bytes([
            bytes[data_end],
            bytes[data_end + 1],
            bytes[data_end + 2],
            bytes[data_end + 3],
        ]);
        let computed_crc = crc32(&bytes[pos + 4..data_end]);
        if stored_crc != computed_crc {
            return Err(PngError::Crc {
                chunk: String::from_utf8_lossy(&kind).into_owned(),
                expected: computed_crc,
                found: stored_crc,
            });
        }

        match &kind {
            b"IHDR" => {
                if header.is_some() {
                    return Err(PngError::Invalid("more than one IHDR chunk".into()));
                }
                header = Some(parse_ihdr(data)?);
            }
            b"PLTE" => palette = Some(parse_plte(data)?),
            b"tRNS" => transparency = Some(data.to_vec()),
            b"IDAT" => {
                if header.is_none() {
                    return Err(PngError::Invalid("IDAT chunk before IHDR".into()));
                }
                if idat.len() + data.len() > MAX_INFLATE_BYTES {
                    return Err(PngError::TooLarge(format!(
                        "image data exceeds {MAX_INFLATE_BYTES} compressed bytes"
                    )));
                }
                idat.extend_from_slice(data);
            }
            b"IEND" => {
                saw_iend = true;
                break;
            }
            other => {
                // Unknown *ancillary* chunks (lowercase first letter) are safe
                // to skip; an unknown critical chunk means we would be guessing.
                if other[0].is_ascii_uppercase() {
                    return Err(PngError::Unsupported(format!(
                        "critical chunk `{}`",
                        String::from_utf8_lossy(other)
                    )));
                }
            }
        }
        pos = crc_end;
    }

    let header = header.ok_or_else(|| PngError::Invalid("missing IHDR chunk".into()))?;
    if !saw_iend {
        return Err(PngError::Truncated { at: bytes.len() });
    }
    if idat.is_empty() {
        return Err(PngError::Invalid("missing IDAT chunk".into()));
    }
    if header.color_type == 3 && palette.is_none() {
        return Err(PngError::Invalid(
            "indexed image without a PLTE chunk".into(),
        ));
    }

    let expected = header.raw_len();
    let limit = expected.min(MAX_INFLATE_BYTES);
    let raw = inflate_zlib(&idat, limit)?;
    if raw.len() != expected {
        return Err(PngError::Invalid(format!(
            "image data decompressed to {} bytes, expected {expected}",
            raw.len()
        )));
    }

    reconstruct(header, palette.as_deref(), transparency.as_deref(), &raw)
}

fn parse_ihdr(data: &[u8]) -> Result<Ihdr, PngError> {
    if data.len() != 13 {
        return Err(PngError::Invalid(format!(
            "IHDR is {} bytes, expected 13",
            data.len()
        )));
    }
    let width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let bit_depth = data[8];
    let color_type = data[9];
    let compression = data[10];
    let filter_method = data[11];
    let interlace = data[12];

    if width == 0 || height == 0 {
        return Err(PngError::Invalid("zero width or height".into()));
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(PngError::TooLarge(format!(
            "{width}x{height} exceeds the {MAX_DIMENSION} pixel dimension limit"
        )));
    }
    if u64::from(width) * u64::from(height) > MAX_DECODED_PIXELS {
        return Err(PngError::TooLarge(format!(
            "{width}x{height} exceeds the {MAX_DECODED_PIXELS} pixel limit"
        )));
    }
    if !valid_depth(color_type, bit_depth) {
        return Err(PngError::Invalid(format!(
            "bit depth {bit_depth} is not allowed for colour type {color_type}"
        )));
    }
    if compression != 0 {
        return Err(PngError::Unsupported(format!(
            "compression method {compression}"
        )));
    }
    if filter_method != 0 {
        return Err(PngError::Unsupported(format!(
            "filter method {filter_method}"
        )));
    }
    match interlace {
        0 => {}
        1 => {
            return Err(PngError::Unsupported(
                "Adam7 interlace: this codec decodes non-interlaced PNGs only".into(),
            ));
        }
        other => return Err(PngError::Invalid(format!("interlace method {other}"))),
    }
    Ok(Ihdr {
        width,
        height,
        bit_depth,
        color_type,
    })
}

fn valid_depth(color_type: u8, bit_depth: u8) -> bool {
    match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        4 | 6 => matches!(bit_depth, 8 | 16),
        _ => false,
    }
}

fn parse_plte(data: &[u8]) -> Result<Vec<Color8>, PngError> {
    if data.is_empty() || data.len() % 3 != 0 {
        return Err(PngError::Invalid(format!(
            "PLTE is {} bytes, expected a multiple of 3",
            data.len()
        )));
    }
    let count = data.len() / 3;
    if count > 256 {
        return Err(PngError::Invalid(format!(
            "PLTE has {count} entries, the maximum is 256"
        )));
    }
    Ok(data
        .chunks_exact(3)
        .map(|rgb| Color8::new(rgb[0], rgb[1], rgb[2], 255))
        .collect())
}

/// Undoes the scanline filters and converts the samples to RGBA.
fn reconstruct(
    header: Ihdr,
    palette: Option<&[Color8]>,
    transparency: Option<&[u8]>,
    raw: &[u8],
) -> Result<Image, PngError> {
    let stride = header.stride();
    let bpp = header.bpp();
    let depth = header.bit_depth;
    let mut pixels = Vec::with_capacity((header.width as usize) * (header.height as usize));
    let mut previous = vec![0u8; stride];

    for y in 0..header.height as usize {
        let row_start = y * (stride + 1);
        let filter = Filter::from_u8(raw[row_start])?;
        let mut row = raw[row_start + 1..row_start + 1 + stride].to_vec();
        unfilter_scanline(filter, bpp, &previous, &mut row);

        for x in 0..header.width as usize {
            let pixel = match header.color_type {
                0 => {
                    let gray = sample_at(&row, x, depth);
                    let alpha = if matches_gray_trns(transparency, gray) {
                        0
                    } else {
                        255
                    };
                    let g = scale_sample(gray, depth);
                    Color8::new(g, g, g, alpha)
                }
                2 => {
                    let r = sample_at(&row, x * 3, depth);
                    let g = sample_at(&row, x * 3 + 1, depth);
                    let b = sample_at(&row, x * 3 + 2, depth);
                    let alpha = if matches_rgb_trns(transparency, r, g, b) {
                        0
                    } else {
                        255
                    };
                    Color8::new(
                        scale_sample(r, depth),
                        scale_sample(g, depth),
                        scale_sample(b, depth),
                        alpha,
                    )
                }
                3 => {
                    let index = sample_at(&row, x, depth) as usize;
                    let table = palette.expect("checked before reconstruct");
                    let entry = table.get(index).ok_or_else(|| {
                        PngError::Invalid(format!("palette index {index} is out of range"))
                    })?;
                    let alpha = transparency
                        .and_then(|t| t.get(index))
                        .copied()
                        .unwrap_or(255);
                    Color8::new(entry.r, entry.g, entry.b, alpha)
                }
                4 => {
                    let gray = sample_at(&row, x * 2, depth);
                    let alpha = sample_at(&row, x * 2 + 1, depth);
                    let g = scale_sample(gray, depth);
                    Color8::new(g, g, g, scale_sample(alpha, depth))
                }
                _ => {
                    let r = sample_at(&row, x * 4, depth);
                    let g = sample_at(&row, x * 4 + 1, depth);
                    let b = sample_at(&row, x * 4 + 2, depth);
                    let a = sample_at(&row, x * 4 + 3, depth);
                    Color8::new(
                        scale_sample(r, depth),
                        scale_sample(g, depth),
                        scale_sample(b, depth),
                        scale_sample(a, depth),
                    )
                }
            };
            pixels.push(pixel);
        }
        previous = row;
    }

    Ok(Image {
        width: header.width,
        height: header.height,
        pixels,
    })
}

/// Reads sample number `index` from an unfiltered scanline.
fn sample_at(row: &[u8], index: usize, depth: u8) -> u16 {
    match depth {
        16 => {
            let i = index * 2;
            (u16::from(row[i]) << 8) | u16::from(row[i + 1])
        }
        8 => u16::from(row[index]),
        d => {
            let per_byte = (8 / d) as usize;
            let byte = row[index / per_byte];
            let shift = 8 - (d as usize) * (index % per_byte) - (d as usize);
            u16::from(byte >> shift) & ((1u16 << d) - 1)
        }
    }
}

/// Expands a sample of `depth` bits to the full 0..=255 range.
fn scale_sample(value: u16, depth: u8) -> u8 {
    match depth {
        16 => (value >> 8) as u8,
        8 => value as u8,
        d => ((u32::from(value) * 255) / ((1u32 << d) - 1)) as u8,
    }
}

fn trns_sample(transparency: Option<&[u8]>, offset: usize) -> Option<u16> {
    let bytes = transparency?;
    let hi = *bytes.get(offset)?;
    let lo = *bytes.get(offset + 1)?;
    Some((u16::from(hi) << 8) | u16::from(lo))
}

fn matches_gray_trns(transparency: Option<&[u8]>, gray: u16) -> bool {
    trns_sample(transparency, 0) == Some(gray)
}

fn matches_rgb_trns(transparency: Option<&[u8]>, r: u16, g: u16, b: u16) -> bool {
    trns_sample(transparency, 0) == Some(r)
        && trns_sample(transparency, 2) == Some(g)
        && trns_sample(transparency, 4) == Some(b)
}

/// Encodes an image as a PNG byte vector.
///
/// The output is always colour type 6, bit depth 8, filter 0, no interlace, so
/// decoding it is lossless for any [`Image`]. An empty image (zero width or
/// height) is written as a single transparent pixel, because the PNG format has
/// no representation for a zero-sized image.
#[must_use]
pub fn encode(image: &Image) -> Vec<u8> {
    let (width, height) = if image.is_empty() {
        (1, 1)
    } else {
        (image.width, image.height)
    };
    let mut raw = Vec::with_capacity(height as usize * (1 + width as usize * 4));
    for y in 0..height {
        raw.push(Filter::None.to_u8());
        for x in 0..width {
            let pixel = image.get_or_transparent(x, y);
            raw.extend_from_slice(&[pixel.r, pixel.g, pixel.b, pixel.a]);
        }
    }
    let compressed = zlib_compress(&raw);

    let mut out = Vec::with_capacity(compressed.len() + 64);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &compressed);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

// ---------------------------------------------------------------------------
// Checksums
// ---------------------------------------------------------------------------

const CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// The CRC-32 used by every PNG chunk.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = CRC_TABLE[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// The Adler-32 checksum used by the zlib container.
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

// ---------------------------------------------------------------------------
// Inflate (DEFLATE decompression)
// ---------------------------------------------------------------------------

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Reads bits least-significant-bit first, which is DEFLATE's bit order.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buffer: u32,
    bits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buffer: 0,
            bits: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u32, PngError> {
        if self.bits == 0 {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or_else(|| PngError::Inflate("unexpected end of deflate stream".into()))?;
            self.pos += 1;
            self.buffer = u32::from(byte);
            self.bits = 8;
        }
        let bit = self.buffer & 1;
        self.buffer >>= 1;
        self.bits -= 1;
        Ok(bit)
    }

    fn read_bits(&mut self, count: u8) -> Result<u32, PngError> {
        let mut value = 0u32;
        for i in 0..count {
            value |= self.read_bit()? << i;
        }
        Ok(value)
    }

    fn align_to_byte(&mut self) {
        self.buffer = 0;
        self.bits = 0;
    }

    fn read_u16_le(&mut self) -> Result<u16, PngError> {
        let lo = self.read_aligned_byte()?;
        let hi = self.read_aligned_byte()?;
        Ok(u16::from(lo) | (u16::from(hi) << 8))
    }

    fn read_u32_be(&mut self) -> Result<u32, PngError> {
        let mut value = 0u32;
        for _ in 0..4 {
            value = (value << 8) | u32::from(self.read_aligned_byte()?);
        }
        Ok(value)
    }

    fn read_aligned_byte(&mut self) -> Result<u8, PngError> {
        self.align_to_byte();
        let byte = *self
            .data
            .get(self.pos)
            .ok_or_else(|| PngError::Inflate("unexpected end of deflate stream".into()))?;
        self.pos += 1;
        Ok(byte)
    }
}

/// A canonical Huffman decoding table.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self, PngError> {
        let mut counts = [0u16; 16];
        for &length in lengths {
            if length > 15 {
                return Err(PngError::Inflate(format!(
                    "Huffman code length {length} is over 15"
                )));
            }
            counts[length as usize] += 1;
        }
        counts[0] = 0;
        // A code that claims more leaves than the tree can hold is corrupt.
        let mut left = 1i32;
        for count in counts.iter().skip(1) {
            left <<= 1;
            left -= i32::from(*count);
            if left < 0 {
                return Err(PngError::Inflate("over-subscribed Huffman code".into()));
            }
        }
        let mut offsets = [0u16; 16];
        for length in 1..15 {
            offsets[length + 1] = offsets[length] + counts[length];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let slot = &mut offsets[length as usize];
                symbols[*slot as usize] = symbol as u16;
                *slot += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, PngError> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for length in 1..16 {
            code |= reader.read_bit()? as i32;
            let count = i32::from(self.counts[length]);
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(PngError::Inflate("invalid Huffman code".into()))
    }
}

fn fixed_tables() -> Result<(Huffman, Huffman), PngError> {
    let mut literal_lengths = [0u8; 288];
    for (symbol, length) in literal_lengths.iter_mut().enumerate() {
        *length = match symbol {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let distance_lengths = [5u8; 30];
    Ok((
        Huffman::new(&literal_lengths)?,
        Huffman::new(&distance_lengths)?,
    ))
}

/// Decompresses a zlib stream whose uncompressed size must not exceed `limit`.
pub fn inflate_zlib(data: &[u8], limit: usize) -> Result<Vec<u8>, PngError> {
    if data.len() < 2 {
        return Err(PngError::Inflate(
            "zlib stream is shorter than its header".into(),
        ));
    }
    let cmf = data[0];
    let flg = data[1];
    if cmf & 0x0F != 8 {
        return Err(PngError::Unsupported(format!(
            "zlib compression method {}",
            cmf & 0x0F
        )));
    }
    if (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return Err(PngError::Inflate("bad zlib header check bits".into()));
    }
    if flg & 0x20 != 0 {
        return Err(PngError::Unsupported("zlib preset dictionary".into()));
    }
    let mut reader = BitReader::new(&data[2..]);
    let out = inflate_raw(&mut reader, limit)?;
    let stored = reader.read_u32_be()?;
    let computed = adler32(&out);
    if stored != computed {
        return Err(PngError::Inflate(format!(
            "adler-32 mismatch: stored {stored:#010x}, computed {computed:#010x}"
        )));
    }
    Ok(out)
}

fn inflate_raw(reader: &mut BitReader<'_>, limit: usize) -> Result<Vec<u8>, PngError> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let is_final = reader.read_bit()? == 1;
        let block_type = reader.read_bits(2)?;
        match block_type {
            0 => {
                reader.align_to_byte();
                let len = reader.read_u16_le()? as usize;
                let nlen = reader.read_u16_le()?;
                if len != usize::from(!nlen) {
                    return Err(PngError::Inflate("stored block length check failed".into()));
                }
                if out.len() + len > limit {
                    return Err(too_large(limit));
                }
                for _ in 0..len {
                    out.push(reader.read_aligned_byte()?);
                }
            }
            1 => {
                let (literals, distances) = fixed_tables()?;
                inflate_block(reader, &literals, &distances, &mut out, limit)?;
            }
            2 => {
                let (literals, distances) = dynamic_tables(reader)?;
                inflate_block(reader, &literals, &distances, &mut out, limit)?;
            }
            _ => return Err(PngError::Inflate("reserved block type 3".into())),
        }
        if is_final {
            return Ok(out);
        }
    }
}

fn too_large(limit: usize) -> PngError {
    PngError::TooLarge(format!("decompressed data exceeds the {limit} byte limit"))
}

fn inflate_block(
    reader: &mut BitReader<'_>,
    literals: &Huffman,
    distances: &Huffman,
    out: &mut Vec<u8>,
    limit: usize,
) -> Result<(), PngError> {
    loop {
        let symbol = literals.decode(reader)?;
        if symbol < 256 {
            if out.len() >= limit {
                return Err(too_large(limit));
            }
            out.push(symbol as u8);
            continue;
        }
        if symbol == 256 {
            return Ok(());
        }
        let index = (symbol - 257) as usize;
        if index >= LENGTH_BASE.len() {
            return Err(PngError::Inflate(format!("invalid length code {symbol}")));
        }
        let length =
            usize::from(LENGTH_BASE[index]) + reader.read_bits(LENGTH_EXTRA[index])? as usize;

        let dist_symbol = distances.decode(reader)? as usize;
        if dist_symbol >= DIST_BASE.len() {
            return Err(PngError::Inflate(format!(
                "invalid distance code {dist_symbol}"
            )));
        }
        let distance = usize::from(DIST_BASE[dist_symbol])
            + reader.read_bits(DIST_EXTRA[dist_symbol])? as usize;
        if distance > out.len() {
            return Err(PngError::Inflate(
                "match distance reaches before the output start".into(),
            ));
        }
        if out.len() + length > limit {
            return Err(too_large(limit));
        }
        // Copies byte by byte on purpose: a match may overlap the bytes it is
        // producing (distance < length), which is how DEFLATE encodes runs of
        // up to the 258-byte maximum match length.
        let start = out.len() - distance;
        for i in 0..length {
            let byte = out[start + i];
            out.push(byte);
        }
    }
}

fn dynamic_tables(reader: &mut BitReader<'_>) -> Result<(Huffman, Huffman), PngError> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let literal_count = reader.read_bits(5)? as usize + 257;
    let distance_count = reader.read_bits(5)? as usize + 1;
    let code_count = reader.read_bits(4)? as usize + 4;
    if literal_count > 286 || distance_count > 30 {
        return Err(PngError::Inflate(
            "too many Huffman codes in a dynamic block".into(),
        ));
    }

    let mut code_lengths = [0u8; 19];
    for &slot in ORDER.iter().take(code_count) {
        code_lengths[slot] = reader.read_bits(3)? as u8;
    }
    let code_table = Huffman::new(&code_lengths)?;

    let mut lengths = vec![0u8; literal_count + distance_count];
    let mut i = 0usize;
    while i < lengths.len() {
        let symbol = code_table.decode(reader)?;
        match symbol {
            0..=15 => {
                lengths[i] = symbol as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err(PngError::Inflate(
                        "repeat code with no previous length".into(),
                    ));
                }
                let previous = lengths[i - 1];
                let repeat = 3 + reader.read_bits(2)? as usize;
                if i + repeat > lengths.len() {
                    return Err(PngError::Inflate(
                        "code length repeat overruns the table".into(),
                    ));
                }
                lengths[i..i + repeat].fill(previous);
                i += repeat;
            }
            17 | 18 => {
                let repeat = if symbol == 17 {
                    3 + reader.read_bits(3)? as usize
                } else {
                    11 + reader.read_bits(7)? as usize
                };
                if i + repeat > lengths.len() {
                    return Err(PngError::Inflate(
                        "code length repeat overruns the table".into(),
                    ));
                }
                lengths[i..i + repeat].fill(0);
                i += repeat;
            }
            other => {
                return Err(PngError::Inflate(format!(
                    "invalid code length symbol {other}"
                )));
            }
        }
    }

    let literals = Huffman::new(&lengths[..literal_count])?;
    let distances = Huffman::new(&lengths[literal_count..])?;
    Ok((literals, distances))
}

// ---------------------------------------------------------------------------
// Deflate (fixed-Huffman compression)
// ---------------------------------------------------------------------------

const WINDOW_SIZE: usize = 32768;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MAX_CHAIN: usize = 128;
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;

/// Writes bits least-significant-bit first into a byte vector.
struct BitWriter {
    out: Vec<u8>,
    byte: u8,
    count: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            byte: 0,
            count: 0,
        }
    }

    fn write_bit(&mut self, bit: u32) {
        if bit & 1 != 0 {
            self.byte |= 1 << self.count;
        }
        self.count += 1;
        if self.count == 8 {
            self.out.push(self.byte);
            self.byte = 0;
            self.count = 0;
        }
    }

    /// Writes the low `count` bits of `value`, least significant bit first.
    fn write_bits(&mut self, value: u32, count: u8) {
        for i in 0..count {
            self.write_bit((value >> i) & 1);
        }
    }

    /// Writes a Huffman code, most significant bit first.
    fn write_code(&mut self, code: u16, count: u8) {
        for i in (0..count).rev() {
            self.write_bit(u32::from((code >> i) & 1));
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push(self.byte);
        }
        self.out
    }
}

/// The fixed literal/length code from RFC 1951 section 3.2.6.
fn fixed_code(symbol: u16) -> (u16, u8) {
    match symbol {
        0..=143 => (0x30 + symbol, 8),
        144..=255 => (0x190 + (symbol - 144), 9),
        256..=279 => (symbol - 256, 7),
        _ => (0xC0 + (symbol - 280), 8),
    }
}

fn length_code(length: usize) -> (u16, u8, u16) {
    let mut index = LENGTH_BASE.len() - 1;
    while index > 0 && usize::from(LENGTH_BASE[index]) > length {
        index -= 1;
    }
    (
        257 + index as u16,
        LENGTH_EXTRA[index],
        (length - usize::from(LENGTH_BASE[index])) as u16,
    )
}

fn distance_code(distance: usize) -> (u16, u8, u16) {
    let mut index = DIST_BASE.len() - 1;
    while index > 0 && usize::from(DIST_BASE[index]) > distance {
        index -= 1;
    }
    (
        index as u16,
        DIST_EXTRA[index],
        (distance - usize::from(DIST_BASE[index])) as u16,
    )
}

fn hash3(data: &[u8], at: usize) -> usize {
    let key =
        (u32::from(data[at]) << 16) | (u32::from(data[at + 1]) << 8) | u32::from(data[at + 2]);
    (key.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

/// Compresses `data` into a zlib stream (header `0x78 0x9C`) using a single
/// fixed-Huffman DEFLATE block.
#[must_use]
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x9C];
    let mut writer = BitWriter::new();
    writer.write_bit(1); // BFINAL
    writer.write_bits(1, 2); // BTYPE = 01, fixed Huffman

    let n = data.len();
    let mut head = vec![u32::MAX; HASH_SIZE];
    let mut prev = vec![u32::MAX; n.max(1)];
    let mut i = 0usize;
    while i < n {
        let mut best_length = 0usize;
        let mut best_distance = 0usize;
        if i + MIN_MATCH <= n {
            let hash = hash3(data, i);
            let mut candidate = head[hash];
            let mut links = 0;
            while candidate != u32::MAX && links < MAX_CHAIN {
                let start = candidate as usize;
                if i - start > WINDOW_SIZE {
                    break;
                }
                let max_length = (n - i).min(MAX_MATCH);
                let mut length = 0usize;
                while length < max_length && data[start + length] == data[i + length] {
                    length += 1;
                }
                if length > best_length {
                    best_length = length;
                    best_distance = i - start;
                    if length == max_length {
                        break;
                    }
                }
                candidate = prev[start];
                links += 1;
            }
            prev[i] = head[hash];
            head[hash] = i as u32;
        }

        if best_length >= MIN_MATCH {
            let (symbol, extra_bits, extra) = length_code(best_length);
            let (code, bits) = fixed_code(symbol);
            writer.write_code(code, bits);
            writer.write_bits(u32::from(extra), extra_bits);
            let (symbol, extra_bits, extra) = distance_code(best_distance);
            writer.write_code(symbol, 5);
            writer.write_bits(u32::from(extra), extra_bits);
            for (offset, slot) in prev[(i + 1)..(i + best_length)].iter_mut().enumerate() {
                let k = i + 1 + offset;
                if k + MIN_MATCH <= n {
                    let hash = hash3(data, k);
                    *slot = head[hash];
                    head[hash] = k as u32;
                }
            }
            i += best_length;
        } else {
            let (code, bits) = fixed_code(u16::from(data[i]));
            writer.write_code(code, bits);
            i += 1;
        }
    }

    let (end_code, end_bits) = fixed_code(256);
    writer.write_code(end_code, end_bits);
    out.extend_from_slice(&writer.finish());
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift noise; the engine's own PRNG lives in
    /// `noxel-core`, which this crate may use, but a two-line generator keeps
    /// the codec tests self-contained.
    fn noise_image(width: u32, height: u32, seed: u32) -> Image {
        let mut state = seed | 1;
        Image::from_fn(width, height, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            Color8::new(
                (state >> 24) as u8,
                (state >> 16) as u8,
                (state >> 8) as u8,
                state as u8,
            )
        })
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_input = Vec::from(*kind);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    /// A zlib stream made of stored (uncompressed) DEFLATE blocks.
    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut offset = 0usize;
        loop {
            let end = (offset + 65535).min(data.len());
            let piece = &data[offset..end];
            let is_last = end == data.len();
            out.push(u8::from(is_last)); // BFINAL, BTYPE = 00, then padding
            let len = piece.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(piece);
            offset = end;
            if is_last {
                break;
            }
        }
        out.extend_from_slice(&adler32(data).to_be_bytes());
        out
    }

    fn paeth(a: u8, b: u8, c: u8) -> u8 {
        let p = i16::from(a) + i16::from(b) - i16::from(c);
        let (pa, pb, pc) = (
            (p - i16::from(a)).abs(),
            (p - i16::from(b)).abs(),
            (p - i16::from(c)).abs(),
        );
        if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        }
    }

    /// Independent filter application, written from the specification rather
    /// than by calling the module's own `filter_scanline`.
    fn apply_filter(kind: u8, bpp: usize, previous: &[u8], current: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; current.len()];
        for i in 0..current.len() {
            let a = if i >= bpp { current[i - bpp] } else { 0 };
            let b = previous.get(i).copied().unwrap_or(0);
            let c = if i >= bpp {
                previous.get(i - bpp).copied().unwrap_or(0)
            } else {
                0
            };
            let predictor = match kind {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                _ => panic!("bad filter"),
            };
            out[i] = current[i].wrapping_sub(predictor);
        }
        out
    }

    fn channels(color_type: u8) -> usize {
        match color_type {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        }
    }

    /// Assembles a PNG from raw (unfiltered) scanlines.
    struct PngBuilder {
        width: u32,
        height: u32,
        bit_depth: u8,
        color_type: u8,
        palette: Vec<[u8; 3]>,
        trns: Vec<u8>,
        rows: Vec<Vec<u8>>,
        filters: Vec<u8>,
        extra: Vec<(&'static [u8; 4], Vec<u8>)>,
        idat_chunks: usize,
        interlace: u8,
    }

    impl PngBuilder {
        fn new(width: u32, height: u32, bit_depth: u8, color_type: u8) -> Self {
            Self {
                width,
                height,
                bit_depth,
                color_type,
                palette: Vec::new(),
                trns: Vec::new(),
                rows: Vec::new(),
                filters: vec![0],
                extra: Vec::new(),
                idat_chunks: 1,
                interlace: 0,
            }
        }

        fn row(mut self, bytes: Vec<u8>) -> Self {
            self.rows.push(bytes);
            self
        }

        fn palette(mut self, entries: Vec<[u8; 3]>) -> Self {
            self.palette = entries;
            self
        }

        fn trns(mut self, bytes: Vec<u8>) -> Self {
            self.trns = bytes;
            self
        }

        fn build(&self) -> Vec<u8> {
            let bpp = (channels(self.color_type) * self.bit_depth as usize)
                .div_ceil(8)
                .max(1);
            let mut raw = Vec::new();
            let mut previous: Vec<u8> = Vec::new();
            for (y, row) in self.rows.iter().enumerate() {
                let filter = self.filters.get(y).copied().unwrap_or(0);
                raw.push(filter);
                raw.extend_from_slice(&apply_filter(filter, bpp, &previous, row));
                previous = row.clone();
            }
            let compressed = zlib_compress(&raw);

            let mut out = Vec::from(SIGNATURE);
            let mut ihdr = Vec::new();
            ihdr.extend_from_slice(&self.width.to_be_bytes());
            ihdr.extend_from_slice(&self.height.to_be_bytes());
            ihdr.extend_from_slice(&[self.bit_depth, self.color_type, 0, 0, self.interlace]);
            chunk(&mut out, b"IHDR", &ihdr);
            if !self.palette.is_empty() {
                let mut plte = Vec::new();
                for entry in &self.palette {
                    plte.extend_from_slice(entry);
                }
                chunk(&mut out, b"PLTE", &plte);
            }
            if !self.trns.is_empty() {
                chunk(&mut out, b"tRNS", &self.trns);
            }
            for (kind, data) in &self.extra {
                chunk(&mut out, kind, data);
            }
            let pieces = self.idat_chunks.max(1);
            let per = compressed.len().div_ceil(pieces).max(1);
            for piece in compressed.chunks(per) {
                chunk(&mut out, b"IDAT", piece);
            }
            chunk(&mut out, b"IEND", &[]);
            out
        }
    }

    /// Rewrites a file's IHDR interlace byte and fixes the chunk CRC.
    fn set_interlace(png: &mut [u8], value: u8) {
        // Signature (8) + length (4) + type (4) => IHDR data starts at 16.
        let ihdr_data = 16;
        png[ihdr_data + 12] = value;
        let crc_at = ihdr_data + 13;
        let crc = crc32(&png[12..ihdr_data + 13]);
        png[crc_at..crc_at + 4].copy_from_slice(&crc.to_be_bytes());
    }

    #[test]
    fn round_trip_is_exact_for_many_shapes() {
        let cases = [
            (1u32, 1u32),
            (16, 16),
            (63, 65),
            (256, 256),
            (1, 300),
            (300, 1),
        ];
        for (w, h) in cases {
            let image = noise_image(w, h, w * 7919 + h);
            let decoded = decode(&encode(&image)).unwrap();
            assert_eq!(decoded, image, "noise {w}x{h}");
        }

        let transparent = Image::new(32, 32, Color8::TRANSPARENT);
        assert_eq!(decode(&encode(&transparent)).unwrap(), transparent);

        let flat = Image::new(17, 9, Color8::rgb(7, 200, 13));
        assert_eq!(decode(&encode(&flat)).unwrap(), flat);

        let gradient = Image::from_fn(64, 64, |x, y| {
            Color8::new(x as u8, y as u8, (x ^ y) as u8, 255 - ((x + y) % 256) as u8)
        });
        assert_eq!(decode(&encode(&gradient)).unwrap(), gradient);
    }

    #[test]
    fn encoder_output_is_a_well_formed_png() {
        let image = noise_image(20, 20, 99);
        let bytes = encode(&image);
        assert_eq!(&bytes[..8], &SIGNATURE);
        assert_eq!(&bytes[8..12], &[0, 0, 0, 13], "IHDR length");
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(bytes[24], 8, "bit depth");
        assert_eq!(bytes[25], 6, "colour type RGBA");
        assert_eq!(bytes[26], 0, "compression");
        assert_eq!(bytes[27], 0, "filter method");
        assert_eq!(bytes[28], 0, "interlace");
        assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
        // Signature (8) + IHDR chunk (25) puts the IDAT header at 33.
        assert_eq!(&bytes[37..41], b"IDAT");
        assert_eq!(&bytes[41..43], &[0x78, 0x9C], "zlib header");
    }

    #[test]
    fn empty_image_encodes_as_one_transparent_pixel() {
        let encoded = encode(&Image::new(0, 0, Color8::RED));
        let decoded = decode(&encoded).unwrap();
        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert_eq!(decoded.get(0, 0), Some(Color8::TRANSPARENT));
    }

    #[test]
    fn decodes_grayscale_at_every_bit_depth() {
        let one = PngBuilder::new(8, 1, 1, 0).row(vec![0b1011_0001]).build();
        let image = decode(&one).unwrap();
        let expected = [255u8, 0, 255, 255, 0, 0, 0, 255];
        for (x, want) in expected.iter().enumerate() {
            assert_eq!(
                image.get(x as u32, 0),
                Some(Color8::rgb(*want, *want, *want))
            );
        }

        let two = PngBuilder::new(4, 1, 2, 0).row(vec![0b00_01_10_11]).build();
        let image = decode(&two).unwrap();
        let expected = [0u8, 85, 170, 255];
        for (x, want) in expected.iter().enumerate() {
            assert_eq!(
                image.get(x as u32, 0),
                Some(Color8::rgb(*want, *want, *want))
            );
        }

        let four = PngBuilder::new(4, 1, 4, 0).row(vec![0x0F, 0x70]).build();
        let image = decode(&four).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::rgb(0, 0, 0)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(255, 255, 255)));
        assert_eq!(image.get(2, 0), Some(Color8::rgb(119, 119, 119)));
        assert_eq!(image.get(3, 0), Some(Color8::rgb(0, 0, 0)));

        let eight = PngBuilder::new(2, 1, 8, 0).row(vec![0, 128]).build();
        let image = decode(&eight).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::rgb(0, 0, 0)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(128, 128, 128)));

        let sixteen = PngBuilder::new(2, 1, 16, 0)
            .row(vec![0x12, 0x34, 0xFF, 0xFF])
            .build();
        let image = decode(&sixteen).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::rgb(0x12, 0x12, 0x12)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(255, 255, 255)));
        assert_eq!(image.get(0, 0).unwrap().a, 255);
    }

    #[test]
    fn grayscale_transparency_key_is_honoured() {
        let png = PngBuilder::new(3, 1, 8, 0)
            .row(vec![10, 20, 10])
            .trns(vec![0, 10])
            .build();
        let image = decode(&png).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::new(10, 10, 10, 0)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(20, 20, 20)));
        assert_eq!(image.get(2, 0), Some(Color8::new(10, 10, 10, 0)));
    }

    #[test]
    fn decodes_rgb_and_rgb16() {
        let rgb = PngBuilder::new(2, 1, 8, 2)
            .row(vec![1, 2, 3, 253, 254, 255])
            .build();
        let image = decode(&rgb).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::rgb(1, 2, 3)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(253, 254, 255)));

        let rgb16 = PngBuilder::new(1, 1, 16, 2)
            .row(vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66])
            .build();
        let image = decode(&rgb16).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::rgb(0x11, 0x33, 0x55)));

        let keyed = PngBuilder::new(2, 1, 8, 2)
            .row(vec![1, 2, 3, 4, 5, 6])
            .trns(vec![0, 1, 0, 2, 0, 3])
            .build();
        let image = decode(&keyed).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::new(1, 2, 3, 0)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(4, 5, 6)));
    }

    #[test]
    fn decodes_indexed_at_every_bit_depth() {
        let palette = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [10, 20, 30]];

        let builder = PngBuilder::new(4, 1, 1, 3)
            .row(vec![0b0101_0000])
            .palette(palette.to_vec());
        let image = decode(&builder.build()).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::rgb(255, 0, 0)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(0, 255, 0)));
        assert_eq!(image.get(2, 0), Some(Color8::rgb(255, 0, 0)));
        assert_eq!(image.get(3, 0), Some(Color8::rgb(0, 255, 0)));

        let builder = PngBuilder::new(4, 1, 2, 3)
            .row(vec![0b00_01_10_11])
            .palette(palette.to_vec());
        let image = decode(&builder.build()).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::rgb(255, 0, 0)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(0, 255, 0)));
        assert_eq!(image.get(2, 0), Some(Color8::rgb(0, 0, 255)));
        assert_eq!(image.get(3, 0), Some(Color8::rgb(10, 20, 30)));

        let builder = PngBuilder::new(4, 1, 4, 3)
            .row(vec![0x01, 0x23])
            .palette(palette.to_vec());
        let image = decode(&builder.build()).unwrap();
        assert_eq!(image.get(1, 0), Some(Color8::rgb(0, 255, 0)));
        assert_eq!(image.get(2, 0), Some(Color8::rgb(0, 0, 255)));

        let builder = PngBuilder::new(4, 1, 8, 3)
            .row(vec![0, 1, 2, 3])
            .palette(palette.to_vec())
            .trns(vec![128]);
        let image = decode(&builder.build()).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::new(255, 0, 0, 128)));
        assert_eq!(image.get(1, 0), Some(Color8::new(0, 255, 0, 255)));
        assert_eq!(image.get(3, 0), Some(Color8::rgb(10, 20, 30)));
    }

    #[test]
    fn decodes_gray_alpha_and_rgba16() {
        let gray_alpha = PngBuilder::new(2, 1, 8, 4)
            .row(vec![40, 255, 80, 0])
            .build();
        let image = decode(&gray_alpha).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::new(40, 40, 40, 255)));
        assert_eq!(image.get(1, 0), Some(Color8::new(80, 80, 80, 0)));

        let gray_alpha16 = PngBuilder::new(1, 1, 16, 4)
            .row(vec![0x12, 0x34, 0xAB, 0xCD])
            .build();
        let image = decode(&gray_alpha16).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::new(0x12, 0x12, 0x12, 0xAB)));

        let rgba16 = PngBuilder::new(1, 1, 16, 6)
            .row(vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88])
            .build();
        let image = decode(&rgba16).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::new(0x11, 0x33, 0x55, 0x77)));
    }

    #[test]
    fn all_five_filters_reconstruct_the_same_pixels() {
        let rows: Vec<Vec<u8>> = (0..4)
            .map(|y: u8| {
                (0..4)
                    .flat_map(|x: u8| {
                        [
                            x.wrapping_mul(40).wrapping_add(1),
                            y.wrapping_mul(50).wrapping_add(2),
                            x.wrapping_add(y).wrapping_mul(20).wrapping_add(3),
                        ]
                    })
                    .collect()
            })
            .collect();
        let expected = Image::from_fn(4, 4, |x, y| {
            Color8::rgb(
                (x as u8).wrapping_mul(40).wrapping_add(1),
                (y as u8).wrapping_mul(50).wrapping_add(2),
                (x as u8)
                    .wrapping_add(y as u8)
                    .wrapping_mul(20)
                    .wrapping_add(3),
            )
        });

        for filter in 0..=4u8 {
            let mut builder = PngBuilder::new(4, 4, 8, 2);
            for row in &rows {
                builder = builder.row(row.clone());
            }
            builder.filters = vec![filter; 4];
            let image = decode(&builder.build()).unwrap();
            assert_eq!(image, expected, "filter {filter}");
        }

        // One image mixing all five filters, row by row.
        let mut builder = PngBuilder::new(4, 4, 8, 2);
        for row in &rows {
            builder = builder.row(row.clone());
        }
        builder.filters = vec![0, 1, 2, 3];
        assert_eq!(decode(&builder.build()).unwrap(), expected);

        let mut builder = PngBuilder::new(4, 1, 8, 2);
        builder = builder.row(rows[0].clone());
        builder.filters = vec![4];
        assert_eq!(
            decode(&builder.build()).unwrap().get(3, 0),
            expected.get(3, 0)
        );

        // Filtered sub-byte data still round trips (bpp is 1 there).
        let mut builder = PngBuilder::new(4, 2, 2, 3)
            .row(vec![0b00_01_10_11])
            .row(vec![0b11_10_01_00]);
        builder.filters = vec![1, 4];
        let builder = builder.palette(vec![[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]]);
        let image = decode(&builder.build()).unwrap();
        assert_eq!(image.get(0, 1), Some(Color8::rgb(10, 11, 12)));
        assert_eq!(image.get(3, 0), Some(Color8::rgb(10, 11, 12)));
    }

    #[test]
    fn filter_helpers_are_inverses() {
        let raw: Vec<u8> = (0..24u8)
            .map(|i| i.wrapping_mul(7).wrapping_add(3))
            .collect();
        let previous: Vec<u8> = (0..12u8).map(|i| 255 - i).collect();
        for filter in [
            Filter::None,
            Filter::Sub,
            Filter::Up,
            Filter::Average,
            Filter::Paeth,
        ] {
            let mut buffer = raw.clone();
            filter_scanline(filter, 4, &previous, &mut buffer);
            if filter != Filter::None {
                assert_ne!(buffer, raw, "{filter:?} should change something");
            }
            unfilter_scanline(filter, 4, &previous, &mut buffer);
            assert_eq!(buffer, raw, "{filter:?}");
            assert_eq!(
                filter.to_u8(),
                Filter::from_u8(filter.to_u8()).unwrap().to_u8()
            );
        }
        assert!(Filter::from_u8(5).is_err());
        assert_eq!(paeth_predictor(10, 20, 15), 15);
    }

    #[test]
    fn multiple_idat_chunks_are_concatenated() {
        let image = noise_image(32, 32, 7);
        let mut builder = PngBuilder::new(32, 32, 8, 6);
        for y in 0..32 {
            let row: Vec<u8> = (0..32)
                .flat_map(|x| image.get(x, y).unwrap().to_array())
                .collect();
            builder = builder.row(row);
        }
        builder.idat_chunks = 5;
        assert_eq!(decode(&builder.build()).unwrap(), image);
    }

    #[test]
    fn unknown_ancillary_chunks_are_skipped() {
        let mut builder = PngBuilder::new(2, 1, 8, 0).row(vec![1, 2]);
        builder.extra.push((b"tEXt", b"Comment\0hello".to_vec()));
        builder.extra.push((b"gAMA", vec![0, 0, 0xB0, 0x18]));
        let image = decode(&builder.build()).unwrap();
        assert_eq!(image.get(0, 0), Some(Color8::rgb(1, 1, 1)));
        assert_eq!(image.get(1, 0), Some(Color8::rgb(2, 2, 2)));
    }

    #[test]
    fn unknown_critical_chunk_is_refused() {
        let mut builder = PngBuilder::new(1, 1, 8, 0).row(vec![7]);
        builder.extra.push((b"ZzZz", vec![1, 2, 3]));
        let err = decode(&builder.build()).unwrap_err();
        assert!(matches!(err, PngError::Unsupported(_)), "{err}");
        assert!(err.to_string().contains("ZzZz"));
    }

    #[test]
    fn crc_mismatch_is_rejected() {
        let mut bytes = encode(&Image::new(4, 4, Color8::WHITE));
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF; // corrupt the IEND CRC
        let err = decode(&bytes).unwrap_err();
        assert!(matches!(err, PngError::Crc { .. }), "{err}");
        assert!(err.to_string().contains("CRC mismatch"));
    }

    #[test]
    fn corrupt_pixel_data_is_rejected() {
        let mut bytes = encode(&Image::new(8, 8, Color8::WHITE));
        // Flip a byte inside the IDAT payload (past the 8+25 byte IHDR chunk).
        let target = 8 + 25 + 8 + 4;
        bytes[target] ^= 0x5A;
        let err = decode(&bytes).unwrap_err();
        assert!(
            matches!(
                err,
                PngError::Crc { .. } | PngError::Inflate(_) | PngError::Invalid(_)
            ),
            "{err}"
        );
    }

    #[test]
    fn truncated_and_malformed_files_are_rejected() {
        let bytes = encode(&Image::new(4, 4, Color8::BLUE));
        for cut in [0usize, 3, 8, 20, 33, bytes.len() - 1] {
            assert!(decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        assert_eq!(decode(b"").unwrap_err(), PngError::Signature);
        assert_eq!(
            decode(b"not a png at all").unwrap_err(),
            PngError::Signature
        );

        // Signature only: no IHDR at all.
        assert!(decode(&SIGNATURE).is_err());

        // A chunk that claims to be longer than the file.
        let mut broken = bytes.clone();
        broken[8..12].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        assert!(matches!(
            decode(&broken).unwrap_err(),
            PngError::Truncated { .. }
        ));
    }

    #[test]
    fn adam7_is_refused_with_a_clear_message() {
        let mut bytes = PngBuilder::new(4, 4, 8, 6)
            .row(vec![0; 16])
            .row(vec![0; 16])
            .row(vec![0; 16])
            .row(vec![0; 16])
            .build();
        set_interlace(&mut bytes, 1);
        let err = decode(&bytes).unwrap_err();
        assert!(matches!(err, PngError::Unsupported(_)), "{err}");
        assert!(err.to_string().contains("Adam7"), "{err}");

        set_interlace(&mut bytes, 2);
        assert!(matches!(decode(&bytes).unwrap_err(), PngError::Invalid(_)));
    }

    #[test]
    fn decode_bomb_is_rejected_without_hanging() {
        // A 4x4 RGBA image needs 4 * (1 + 16) = 68 bytes of image data, but the
        // stream below expands to a megabyte.
        let mut out = Vec::from(SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        let bomb = zlib_stored(&vec![0u8; 1024 * 1024]);
        chunk(&mut out, b"IDAT", &bomb);
        chunk(&mut out, b"IEND", &[]);
        let err = decode(&out).unwrap_err();
        assert!(matches!(err, PngError::TooLarge(_)), "{err}");
        assert!(err.to_string().contains("limit"));
    }

    #[test]
    fn oversized_dimensions_are_rejected() {
        let mut out = Vec::from(SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(MAX_DIMENSION + 1).to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &zlib_compress(&[0, 0, 0, 0, 0]));
        chunk(&mut out, b"IEND", &[]);
        assert!(matches!(decode(&out).unwrap_err(), PngError::TooLarge(_)));

        let mut zero = Vec::from(SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&0u32.to_be_bytes());
        ihdr.extend_from_slice(&0u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut zero, b"IHDR", &ihdr);
        assert!(matches!(decode(&zero).unwrap_err(), PngError::Invalid(_)));

        let mut bad_depth = Vec::from(SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[3, 2, 0, 0, 0]);
        chunk(&mut bad_depth, b"IHDR", &ihdr);
        assert!(decode(&bad_depth).is_err());
    }

    #[test]
    fn missing_palette_and_short_data_are_rejected() {
        // Indexed colour type without PLTE.
        let builder = PngBuilder::new(2, 1, 8, 3)
            .row(vec![0, 1])
            .palette(vec![[1, 2, 3]]);
        let mut bytes = builder.build();
        // Strip the PLTE chunk (IHDR ends at 33, PLTE is 12 + 3 bytes).
        let plte_end = 33 + 12 + 3;
        bytes.drain(33..plte_end);
        let err = decode(&bytes).unwrap_err();
        assert!(matches!(err, PngError::Invalid(_)), "{err}");

        // An image whose data is too short for its declared size.
        let mut out = Vec::from(SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&8u32.to_be_bytes());
        ihdr.extend_from_slice(&8u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &zlib_compress(&[0, 0, 0, 0, 0]));
        chunk(&mut out, b"IEND", &[]);
        assert!(matches!(decode(&out).unwrap_err(), PngError::Invalid(_)));
    }

    #[test]
    fn stored_blocks_and_fixed_blocks_inflate() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let stored = zlib_stored(&data);
        assert_eq!(inflate_zlib(&stored, data.len()).unwrap(), data);
        assert!(matches!(
            inflate_zlib(&stored, 1000).unwrap_err(),
            PngError::TooLarge(_)
        ));

        let compressed = zlib_compress(&data);
        assert_eq!(inflate_zlib(&compressed, data.len()).unwrap(), data);

        // A fixed-Huffman stream carrying overlapping matches (a long run).
        let run = vec![0xABu8; 4096];
        let compressed = zlib_compress(&run);
        assert!(
            compressed.len() < run.len() / 4,
            "a run must actually compress"
        );
        assert_eq!(inflate_zlib(&compressed, run.len()).unwrap(), run);

        let empty = zlib_compress(&[]);
        assert_eq!(inflate_zlib(&empty, 0).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn checksums_match_known_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn bad_zlib_headers_are_rejected() {
        assert!(matches!(
            inflate_zlib(&[], 10).unwrap_err(),
            PngError::Inflate(_)
        ));
        assert!(matches!(
            inflate_zlib(&[0x78], 10).unwrap_err(),
            PngError::Inflate(_)
        ));
        // Wrong compression method.
        assert!(matches!(
            inflate_zlib(&[0x77, 0x9C, 0x00], 10).unwrap_err(),
            PngError::Unsupported(_)
        ));
        // Corrupt header check bits.
        assert!(matches!(
            inflate_zlib(&[0x78, 0x9D, 0x00], 10).unwrap_err(),
            PngError::Inflate(_)
        ));
        // Preset dictionary flag.
        assert!(matches!(
            inflate_zlib(&[0x78, 0xBB, 0x00], 10).unwrap_err(),
            PngError::Unsupported(_)
        ));
        // Valid header, truncated body.
        let mut stream = zlib_compress(b"hello world");
        stream.truncate(stream.len() - 2);
        assert!(inflate_zlib(&stream, 100).is_err());
    }

    #[test]
    fn adler_mismatch_is_detected() {
        let mut stream = zlib_compress(b"the quick brown fox");
        let last = stream.len() - 1;
        stream[last] ^= 0x01;
        let err = inflate_zlib(&stream, 100).unwrap_err();
        assert!(err.to_string().contains("adler-32"), "{err}");
    }

    #[test]
    fn compression_actually_compresses_repetitive_data() {
        let image = Image::new(128, 128, Color8::rgb(3, 4, 5));
        let bytes = encode(&image);
        assert!(bytes.len() < 2048, "flat image took {} bytes", bytes.len());
        assert_eq!(decode(&bytes).unwrap(), image);
    }

    #[test]
    fn error_display_strings_are_informative() {
        let err = PngError::Truncated { at: 42 };
        assert!(err.to_string().contains("42"));
        let err = PngError::Crc {
            chunk: "IDAT".into(),
            expected: 1,
            found: 2,
        };
        assert!(err.to_string().contains("IDAT"));
        assert!(PngError::Signature.to_string().contains("signature"));
        assert!(PngError::Invalid("x".into()).to_string().contains('x'));
        assert!(PngError::TooLarge("big".into()).to_string().contains("big"));
        let _: &dyn std::error::Error = &PngError::Signature;
    }
}
