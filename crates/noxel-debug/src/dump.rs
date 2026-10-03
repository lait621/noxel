//! Headless frame dumping and image comparison helpers.
//!
//! The whole reason Noxel renders on the CPU is that a frame is a
//! `Vec<u8>` you can assert on. This module turns that into a workflow:
//! dump frames from a headless run, compare them across commits, and keep a
//! golden set in the repository.
//!
//! ```
//! use noxel_debug::dump::{FrameDumper, DumpFormat};
//! use noxel_render::Framebuffer;
//!
//! let dir = std::env::temp_dir().join("noxel-dump-doc");
//! let mut dumper = FrameDumper::new(&dir, DumpFormat::Png).unwrap();
//! let mut fb = Framebuffer::new(8, 4);
//! fb.clear([0.5, 0.25, 0.0]);
//! let path = dumper.dump(&fb, 0).unwrap();
//! assert!(path.exists());
//! let _ = std::fs::remove_dir_all(&dir);
//! ```

use std::path::{Path, PathBuf};

use noxel_asset::image::Image;
use noxel_asset::png;
use noxel_render::framebuffer::{Framebuffer, ResolveSettings};

/// How a dumped frame is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DumpFormat {
    /// A PNG, for looking at.
    Png,
    /// A PNG plus a text table of the depth buffer, for a precise diff.
    PngAndDepth,
    /// Raw little-endian `u32` RGBA, for a byte-exact comparison.
    Raw,
}

impl DumpFormat {
    /// The file extension.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png | Self::PngAndDepth => "png",
            Self::Raw => "rgba",
        }
    }
}

/// Writes frames to a directory.
#[derive(Clone, Debug)]
pub struct FrameDumper {
    directory: PathBuf,
    format: DumpFormat,
    resolve: ResolveSettings,
    dumped: u64,
}

impl FrameDumper {
    /// Creates a dumper, creating the directory if needed.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the directory cannot be created.
    pub fn new(directory: impl Into<PathBuf>, format: DumpFormat) -> std::io::Result<Self> {
        let directory = directory.into();
        std::fs::create_dir_all(&directory)?;
        Ok(Self {
            directory,
            format,
            resolve: ResolveSettings::default(),
            dumped: 0,
        })
    }

    /// Creates a dumper on the standard `frames/` directory beside the project.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the directory cannot be created.
    pub fn standard(format: DumpFormat) -> std::io::Result<Self> {
        Self::new(PathBuf::from("frames"), format)
    }

    /// Overrides the resolve settings used to turn the linear buffer into sRGB.
    pub fn set_resolve(&mut self, resolve: ResolveSettings) {
        self.resolve = resolve;
    }

    /// The directory frames land in.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// How many frames have been written.
    #[must_use]
    pub fn dumped(&self) -> u64 {
        self.dumped
    }

    /// Writes a frame, named `frame_000123.png`.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the file cannot be written.
    pub fn dump(&mut self, framebuffer: &Framebuffer, frame: u64) -> std::io::Result<PathBuf> {
        let image = framebuffer.resolve(&self.resolve);
        let path = self
            .directory
            .join(format!("frame_{frame:06}.{}", self.format.extension()));
        match self.format {
            DumpFormat::Png | DumpFormat::PngAndDepth => {
                let bytes = png::encode(&image);
                std::fs::write(&path, bytes)?;
                if self.format == DumpFormat::PngAndDepth {
                    let depth_path = self.directory.join(format!("frame_{frame:06}.depth.txt"));
                    std::fs::write(depth_path, depth_table(framebuffer))?;
                }
            }
            DumpFormat::Raw => {
                let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
                for p in &image.pixels {
                    bytes.extend_from_slice(&[p.r, p.g, p.b, p.a]);
                }
                std::fs::write(&path, bytes)?;
            }
        }
        self.dumped += 1;
        Ok(path)
    }

    /// Writes an image directly, without going through a framebuffer.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the file cannot be written.
    pub fn dump_image(&mut self, image: &Image, name: &str) -> std::io::Result<PathBuf> {
        let path = self.directory.join(format!("{name}.png"));
        std::fs::write(&path, png::encode(image))?;
        self.dumped += 1;
        Ok(path)
    }

    /// Removes every `frame_*.png` (and its depth table) from the directory.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the directory cannot be read.
    pub fn clear(&mut self) -> std::io::Result<usize> {
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("frame_") {
                std::fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Renders the depth buffer as a text table, one row per line.
///
/// A text table diffs well: `git diff` on two of these shows exactly which
/// pixels moved, which a PNG diff cannot.
#[must_use]
pub fn depth_table(framebuffer: &Framebuffer) -> String {
    use core::fmt::Write as _;
    let mut out = String::with_capacity(framebuffer.pixel_count() * 8);
    for y in 0..framebuffer.height() {
        for x in 0..framebuffer.width() {
            let d = framebuffer.depth_at(x, y).unwrap_or(1.0);
            if x > 0 {
                out.push(' ');
            }
            // Four significant digits is more than enough to catch a regression
            // and keeps the file small enough to review.
            let _ = write!(out, "{:.4}", d);
        }
        out.push('\n');
    }
    out
}

/// The result of comparing two frames.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImageDiff {
    /// Number of pixels that differ at all.
    pub differing: u64,
    /// Total pixels compared.
    pub total: u64,
    /// The largest per-channel difference seen.
    pub max_channel_delta: u8,
    /// Sum of squared differences over all channels, for a perceptual score.
    pub squared_error: u64,
}

impl ImageDiff {
    /// True when the images are byte-identical.
    #[must_use]
    pub fn is_identical(&self) -> bool {
        self.differing == 0
    }

    /// The fraction of pixels that differ.
    #[must_use]
    pub fn ratio(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.differing as f32 / self.total as f32
        }
    }

    /// The root-mean-square error per channel.
    #[must_use]
    pub fn rmse(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.squared_error as f32 / (self.total as f32 * 3.0)).sqrt()
    }

    /// True when the difference is within the given tolerance.
    #[must_use]
    pub fn within(&self, max_ratio: f32, max_channel_delta: u8) -> bool {
        self.ratio() <= max_ratio && self.max_channel_delta <= max_channel_delta
    }

    /// A line for a CI log.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} of {} pixels differ ({:.3}%), max channel delta {}, rmse {:.3}",
            self.differing,
            self.total,
            self.ratio() * 100.0,
            self.max_channel_delta,
            self.rmse()
        )
    }
}

/// Compares two images pixel by pixel.
#[must_use]
pub fn compare(a: &Image, b: &Image) -> ImageDiff {
    let mut diff = ImageDiff {
        total: a.pixels.len().max(b.pixels.len()) as u64,
        ..Default::default()
    };
    if a.width != b.width || a.height != b.height {
        // Different sizes cannot be compared meaningfully; report every pixel of
        // the larger image as differing so a size change fails a golden test.
        diff.differing = diff.total;
        diff.max_channel_delta = 255;
        return diff;
    }
    for (pa, pb) in a.pixels.iter().zip(b.pixels.iter()) {
        let d = [
            pa.r.abs_diff(pb.r),
            pa.g.abs_diff(pb.g),
            pa.b.abs_diff(pb.b),
            pa.a.abs_diff(pb.a),
        ];
        let max = *d.iter().max().unwrap_or(&0);
        if max > 0 {
            diff.differing += 1;
            diff.max_channel_delta = diff.max_channel_delta.max(max);
        }
        diff.squared_error += d.iter().map(|v| (*v as u64).pow(2)).sum::<u64>();
    }
    diff
}

/// Compares two files on disk as PNG images.
///
/// # Errors
/// Returns a message when either file cannot be read or decoded.
pub fn compare_files(a: &Path, b: &Path) -> Result<ImageDiff, String> {
    let load = |path: &Path| -> Result<Image, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        png::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    };
    Ok(compare(&load(a)?, &load(b)?))
}

/// Builds a side-by-side comparison image: `a` on the left, `b` on the right,
/// with differing pixels of `a` tinted magenta.
#[must_use]
pub fn diff_image(a: &Image, b: &Image) -> Image {
    let width = a.width * 2 + 1;
    let height = a.height.max(b.height);
    let mut out = Image::new(width, height, noxel_core::math::Color8::new(0, 0, 0, 255));
    for y in 0..a.height {
        for x in 0..a.width {
            if let Some(p) = a.get(x, y) {
                out.set(x, y, p);
            }
            if let Some(p) = b.get(x, y) {
                out.set(x + a.width + 1, y, p);
            }
            if let Some((pa, pb)) = a.get(x, y).zip(b.get(x, y)) {
                if pa != pb {
                    out.set(x, y, noxel_core::math::Color8::new(255, 0, 255, 255));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_core::math::Color8;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noxel-dump-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn framebuffer() -> Framebuffer {
        let mut fb = Framebuffer::new(8, 4);
        fb.clear([0.2, 0.4, 0.6]);
        fb
    }

    #[test]
    fn dump_writes_a_readable_png() {
        let dir = temp_dir("png");
        let mut dumper = FrameDumper::new(&dir, DumpFormat::Png).unwrap();
        let path = dumper.dump(&framebuffer(), 7).unwrap();
        assert_eq!(path.file_name().unwrap(), "frame_000007.png");
        let bytes = std::fs::read(&path).unwrap();
        let image = png::decode(&bytes).unwrap();
        assert_eq!((image.width, image.height), (8, 4));
        assert_eq!(dumper.dumped(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dump_and_depth_writes_both_files() {
        let dir = temp_dir("depth");
        let mut dumper = FrameDumper::new(&dir, DumpFormat::PngAndDepth).unwrap();
        dumper.dump(&framebuffer(), 1).unwrap();
        assert!(dir.join("frame_000001.png").exists());
        assert!(dir.join("frame_000001.depth.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dump_raw_writes_four_bytes_per_pixel() {
        let dir = temp_dir("raw");
        let mut dumper = FrameDumper::new(&dir, DumpFormat::Raw).unwrap();
        let path = dumper.dump(&framebuffer(), 0).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 8 * 4 * 4);
        assert_eq!(DumpFormat::Raw.extension(), "rgba");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dumping_the_same_frame_twice_is_byte_identical() {
        let dir = temp_dir("deterministic");
        let mut dumper = FrameDumper::new(&dir, DumpFormat::Png).unwrap();
        let fb = framebuffer();
        let a = dumper.dump(&fb, 0).unwrap();
        let first = std::fs::read(&a).unwrap();
        let b = dumper.dump(&fb, 1).unwrap();
        let second = std::fs::read(&b).unwrap();
        assert_eq!(
            first, second,
            "the same pixels must encode to the same bytes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_removes_only_dumped_frames() {
        let dir = temp_dir("clear");
        let mut dumper = FrameDumper::new(&dir, DumpFormat::Png).unwrap();
        dumper.dump(&framebuffer(), 0).unwrap();
        std::fs::write(dir.join("keepme.txt"), b"x").unwrap();
        let removed = dumper.clear().unwrap();
        assert_eq!(removed, 1);
        assert!(dir.join("keepme.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn depth_table_has_one_line_per_row() {
        let table = depth_table(&framebuffer());
        assert_eq!(table.lines().count(), 4);
        assert_eq!(table.lines().next().unwrap().split_whitespace().count(), 8);
    }

    #[test]
    fn compare_detects_identical_images() {
        let a = Image::new(4, 4, Color8::GREEN);
        let b = Image::new(4, 4, Color8::GREEN);
        let d = compare(&a, &b);
        assert!(d.is_identical());
        assert_eq!(d.ratio(), 0.0);
        assert_eq!(d.rmse(), 0.0);
        assert!(d.within(0.0, 0));
    }

    #[test]
    fn compare_counts_differing_pixels_and_max_delta() {
        let a = Image::new(4, 4, Color8::new(10, 10, 10, 255));
        let mut b = a.clone();
        b.set(0, 0, Color8::new(10, 10, 15, 255));
        b.set(1, 0, Color8::new(10, 10, 9, 255));
        let d = compare(&a, &b);
        assert_eq!(d.differing, 2);
        assert_eq!(d.max_channel_delta, 5);
        assert!(d.ratio() > 0.0);
        assert!(!d.is_identical());
        assert!(d.summary().contains("differ"));
    }

    #[test]
    fn compare_rejects_different_sizes() {
        let a = Image::new(4, 4, Color8::GREEN);
        let b = Image::new(8, 4, Color8::GREEN);
        let d = compare(&a, &b);
        assert!(!d.is_identical());
        assert_eq!(d.max_channel_delta, 255);
    }

    #[test]
    fn compare_within_tolerance() {
        let a = Image::new(10, 10, Color8::new(100, 100, 100, 255));
        let mut b = a.clone();
        b.set(0, 0, Color8::new(102, 100, 100, 255));
        let d = compare(&a, &b);
        assert!(d.within(0.05, 2));
        assert!(!d.within(0.0, 2));
        assert!(!d.within(0.05, 1));
    }

    #[test]
    fn compare_files_round_trips_through_png() {
        let dir = temp_dir("compare");
        std::fs::create_dir_all(&dir).unwrap();
        let a = Image::new(4, 2, Color8::new(1, 2, 3, 255));
        let b = Image::new(4, 2, Color8::new(1, 2, 3, 255));
        let pa = dir.join("a.png");
        let pb = dir.join("b.png");
        std::fs::write(&pa, png::encode(&a)).unwrap();
        std::fs::write(&pb, png::encode(&b)).unwrap();
        assert!(compare_files(&pa, &pb).unwrap().is_identical());
        assert!(compare_files(&pa, &dir.join("missing.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_image_is_side_by_side_and_marks_differences() {
        let a = Image::new(4, 4, Color8::new(0, 0, 0, 255));
        let mut b = Image::new(4, 4, Color8::new(0, 0, 0, 255));
        b.set(0, 0, Color8::new(255, 255, 255, 255));
        let d = diff_image(&a, &b);
        assert_eq!(d.width, 9);
        assert_eq!(d.get(0, 0).unwrap(), Color8::new(255, 0, 255, 255));
        assert_eq!(d.get(5, 0).unwrap(), Color8::new(255, 255, 255, 255));
    }

    #[test]
    fn dump_image_writes_a_named_file() {
        let dir = temp_dir("named");
        let mut dumper = FrameDumper::new(&dir, DumpFormat::Png).unwrap();
        let path = dumper
            .dump_image(&Image::new(2, 2, Color8::RED), "contact_sheet")
            .unwrap();
        assert!(path.ends_with("contact_sheet.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dumper_creates_missing_directories() {
        let dir = temp_dir("nested").join("a").join("b");
        let dumper = FrameDumper::new(&dir, DumpFormat::Png).unwrap();
        assert!(dumper.directory().exists());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn standard_dumper_targets_the_frames_directory() {
        let dumper = FrameDumper::standard(DumpFormat::Png).unwrap();
        assert_eq!(dumper.directory(), Path::new("frames"));
    }
}
