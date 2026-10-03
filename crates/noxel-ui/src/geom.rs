//! Integer screen geometry.
//!
//! A UI is laid out in **whole framebuffer pixels**, and every type here keeps
//! that promise: positions and sizes are `i32`, not `f32`. Noxel renders at an
//! internal resolution and upscales by a whole number, so a rectangle at
//! `x = 10.5` would land half a pixel off the grid and come out of the upscale
//! as a two-pixel smear. Rounding at the last moment is not enough — by then the
//! half pixel is inside a child that has already been measured against it — so
//! the arithmetic itself is integral.
//!
//! `noxel_core::math::Rect` is the right type for the *world*: it is `f32`, and
//! it has to be, because a camera is never exactly on a pixel. This is the same
//! idea one layer up, where the answer is always a pixel.

use core::fmt;

/// A rectangle in framebuffer pixels.
///
/// The origin is the top-left of the framebuffer, `+x` right and `+y` down,
/// which matches how a framebuffer is indexed and how text is read. It is *not*
/// the world's coordinate system (`+Y` up); a UI that inherited that convention
/// would have to flip every rectangle it drew.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct UiRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl UiRect {
    /// An empty rectangle at the origin.
    pub const ZERO: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    /// Builds a rectangle from a corner and a size.
    #[must_use]
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Builds a rectangle from two corners, in any order.
    #[must_use]
    pub fn from_corners(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let (lx, hx) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        let (ly, hy) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        Self {
            x: lx,
            y: ly,
            w: (hx - lx).max(0) as u32,
            h: (hy - ly).max(0) as u32,
        }
    }

    /// A rectangle covering a whole framebuffer.
    #[must_use]
    pub const fn screen(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            w: width,
            h: height,
        }
    }

    /// The x coordinate one past the right edge.
    #[must_use]
    pub const fn right(&self) -> i32 {
        self.x + self.w as i32
    }

    /// The y coordinate one past the bottom edge.
    #[must_use]
    pub const fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }

    /// The centre, rounded down.
    #[must_use]
    pub const fn center(&self) -> (i32, i32) {
        (self.x + self.w as i32 / 2, self.y + self.h as i32 / 2)
    }

    /// Whether the rectangle covers no pixels.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// The area in pixels.
    #[must_use]
    pub const fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }

    /// Moves the rectangle.
    #[must_use]
    pub const fn translate(&self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }

    /// Moves the rectangle so its top-left is at `(x, y)`.
    #[must_use]
    pub const fn at(&self, x: i32, y: i32) -> Self {
        Self { x, y, ..*self }
    }

    /// Sets the size, keeping the top-left.
    #[must_use]
    pub const fn with_size(&self, w: u32, h: u32) -> Self {
        Self { w, h, ..*self }
    }

    /// Shrinks the rectangle by an inset on each edge.
    ///
    /// A rectangle can be inset out of existence; the result is empty rather
    /// than negative, so a caller never has to check before drawing.
    #[must_use]
    pub fn inset(&self, insets: Insets) -> Self {
        let x = self.x + insets.left;
        let y = self.y + insets.top;
        let w = (self.w as i32 - insets.left - insets.right).max(0) as u32;
        let h = (self.h as i32 - insets.top - insets.bottom).max(0) as u32;
        Self { x, y, w, h }
    }

    /// Grows the rectangle by an outset on each edge.
    #[must_use]
    pub fn outset(&self, insets: Insets) -> Self {
        self.inset(Insets {
            left: -insets.left,
            top: -insets.top,
            right: -insets.right,
            bottom: -insets.bottom,
        })
    }

    /// The overlap of two rectangles, empty when they do not intersect.
    #[must_use]
    pub fn intersect(&self, other: Self) -> Self {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            Self::ZERO
        } else {
            Self {
                x: x0,
                y: y0,
                w: (x1 - x0) as u32,
                h: (y1 - y0) as u32,
            }
        }
    }

    /// The smallest rectangle containing both.
    #[must_use]
    pub fn union(&self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return *self;
        }
        Self::from_corners(
            self.x.min(other.x),
            self.y.min(other.y),
            self.right().max(other.right()),
            self.bottom().max(other.bottom()),
        )
    }

    /// Whether a point is inside. The right and bottom edges are exclusive.
    #[must_use]
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Whether any pixel is shared with another rectangle.
    #[must_use]
    pub fn overlaps(&self, other: Self) -> bool {
        !self.intersect(other).is_empty()
    }

    /// Splits off a horizontal strip `height` tall from the top.
    #[must_use]
    pub fn cut_top(&mut self, height: u32) -> Self {
        let taken = height.min(self.h);
        let strip = Self {
            x: self.x,
            y: self.y,
            w: self.w,
            h: taken,
        };
        self.y += taken as i32;
        self.h -= taken;
        strip
    }

    /// Splits off a horizontal strip `height` tall from the bottom.
    #[must_use]
    pub fn cut_bottom(&mut self, height: u32) -> Self {
        let taken = height.min(self.h);
        self.h -= taken;
        Self {
            x: self.x,
            y: self.bottom(),
            w: self.w,
            h: taken,
        }
    }

    /// Splits off a vertical strip `width` wide from the left.
    #[must_use]
    pub fn cut_left(&mut self, width: u32) -> Self {
        let taken = width.min(self.w);
        let strip = Self {
            x: self.x,
            y: self.y,
            w: taken,
            h: self.h,
        };
        self.x += taken as i32;
        self.w -= taken;
        strip
    }

    /// Splits off a vertical strip `width` wide from the right.
    #[must_use]
    pub fn cut_right(&mut self, width: u32) -> Self {
        let taken = width.min(self.w);
        self.w -= taken;
        Self {
            x: self.right(),
            y: self.y,
            w: taken,
            h: self.h,
        }
    }

    /// Divides into `count` equal columns, left to right.
    ///
    /// The remainder is spread over the first columns so the pieces tile the
    /// rectangle exactly: `UiRect::new(0, 0, 10, 1).columns(3)` is `4 + 3 + 3`,
    /// and the three widths add up to ten. Handing out `10 / 3 = 3` to each and
    /// dropping the last pixel is how a grid ends up one pixel narrower than its
    /// own frame.
    #[must_use]
    pub fn columns(&self, count: u32) -> Vec<Self> {
        self.split(count, true)
    }

    /// Divides into `count` equal rows, top to bottom.
    #[must_use]
    pub fn rows(&self, count: u32) -> Vec<Self> {
        self.split(count, false)
    }

    fn split(&self, count: u32, horizontal: bool) -> Vec<Self> {
        if count == 0 {
            return Vec::new();
        }
        let total = if horizontal { self.w } else { self.h };
        let base = total / count;
        let remainder = total % count;
        let mut out = Vec::with_capacity(count as usize);
        let mut offset = 0u32;
        for index in 0..count {
            let size = base + u32::from(index < remainder);
            out.push(if horizontal {
                Self {
                    x: self.x + offset as i32,
                    y: self.y,
                    w: size,
                    h: self.h,
                }
            } else {
                Self {
                    x: self.x,
                    y: self.y + offset as i32,
                    w: self.w,
                    h: size,
                }
            });
            offset += size;
        }
        out
    }

    /// Stacks `count` rows of a fixed `height`, top to bottom.
    #[must_use]
    pub fn stack(&self, count: u32, height: u32, gap: u32) -> Vec<Self> {
        (0..count)
            .map(|index| {
                let y = self.y + (index * (height + gap)) as i32;
                Self {
                    x: self.x,
                    y,
                    w: self.w,
                    h: height.min(self.h),
                }
            })
            .collect()
    }

    /// Places a rectangle of `size` inside this one according to `anchor`,
    /// optionally nudged by an offset.
    #[must_use]
    pub fn place(&self, size: (u32, u32), anchor: Anchor, offset: (i32, i32)) -> Self {
        let (w, h) = size;
        let x = match anchor.horizontal() {
            Axis::Start => self.x,
            Axis::Center => self.x + (self.w as i32 - w as i32) / 2,
            Axis::End => self.right() - w as i32,
        };
        let y = match anchor.vertical() {
            Axis::Start => self.y,
            Axis::Center => self.y + (self.h as i32 - h as i32) / 2,
            Axis::End => self.bottom() - h as i32,
        };
        Self::new(x + offset.0, y + offset.1, w, h)
    }
}

impl fmt::Display for UiRect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}+{},{}", self.w, self.h, self.x, self.y)
    }
}

/// Which side of a rectangle an anchor or an alignment refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    /// Left or top.
    Start,
    /// Horizontally or vertically centred.
    Center,
    /// Right or bottom.
    End,
}

/// One of the nine positions inside a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Anchor {
    /// Top-left corner.
    TopLeft,
    /// Top edge, centred.
    TopCenter,
    /// Top-right corner.
    TopRight,
    /// Left edge, centred.
    MiddleLeft,
    /// Dead centre.
    Center,
    /// Right edge, centred.
    MiddleRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom edge, centred.
    BottomCenter,
    /// Bottom-right corner.
    BottomRight,
}

impl Anchor {
    /// The horizontal component.
    #[must_use]
    pub const fn horizontal(self) -> Axis {
        match self {
            Self::TopLeft | Self::MiddleLeft | Self::BottomLeft => Axis::Start,
            Self::TopCenter | Self::Center | Self::BottomCenter => Axis::Center,
            Self::TopRight | Self::MiddleRight | Self::BottomRight => Axis::End,
        }
    }

    /// The vertical component.
    #[must_use]
    pub const fn vertical(self) -> Axis {
        match self {
            Self::TopLeft | Self::TopCenter | Self::TopRight => Axis::Start,
            Self::MiddleLeft | Self::Center | Self::MiddleRight => Axis::Center,
            Self::BottomLeft | Self::BottomCenter | Self::BottomRight => Axis::End,
        }
    }
}

/// Space taken off each edge of a rectangle.
///
/// Used for both outer margins and inner padding; the type does not distinguish
/// them because the arithmetic is identical and a name that lied about which one
/// it was would be worse than one name for both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Insets {
    /// Space on the left.
    pub left: i32,
    /// Space on the top.
    pub top: i32,
    /// Space on the right.
    pub right: i32,
    /// Space on the bottom.
    pub bottom: i32,
}

impl Insets {
    /// No space on any edge.
    pub const ZERO: Self = Self {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };

    /// The same space on every edge.
    #[must_use]
    pub const fn all(value: i32) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }

    /// Space on the horizontal edges only.
    #[must_use]
    pub const fn symmetric(horizontal: i32, vertical: i32) -> Self {
        Self {
            left: horizontal,
            top: vertical,
            right: horizontal,
            bottom: vertical,
        }
    }

    /// Distinct horizontal and vertical edges.
    #[must_use]
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The horizontal space this costs: left plus right.
    #[must_use]
    pub const fn horizontal(&self) -> i32 {
        self.left + self.right
    }

    /// The vertical space this costs: top plus bottom.
    #[must_use]
    pub const fn vertical(&self) -> i32 {
        self.top + self.bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_tile_the_rectangle_exactly() {
        // 10 does not divide by 3; the remainder has to go somewhere or the
        // pieces stop covering the rectangle they were cut from.
        let rect = UiRect::new(0, 0, 10, 4);
        let columns = rect.columns(3);
        assert_eq!(
            columns.iter().map(|c| c.w).collect::<Vec<_>>(),
            vec![4, 3, 3]
        );
        assert_eq!(columns.iter().map(|c| c.w).sum::<u32>(), rect.w);
        assert_eq!(columns[0].x, 0);
        assert_eq!(columns[2].right(), rect.right());
    }

    #[test]
    fn rows_tile_the_rectangle_exactly() {
        let rect = UiRect::new(2, 3, 6, 7);
        let rows = rect.rows(3);
        assert_eq!(rows.iter().map(|r| r.h).sum::<u32>(), rect.h);
        assert_eq!(rows[0].y, 3);
        assert_eq!(rows[2].bottom(), rect.bottom());
    }

    #[test]
    fn insetting_past_the_edge_gives_an_empty_rectangle_not_a_negative_one() {
        let rect = UiRect::new(0, 0, 4, 4);
        let squashed = rect.inset(Insets::all(10));
        assert!(squashed.is_empty());
        assert_eq!(squashed.w, 0);
        assert_eq!(squashed.h, 0);
    }

    #[test]
    fn intersection_and_overlap_agree() {
        let a = UiRect::new(0, 0, 10, 10);
        let b = UiRect::new(8, 8, 10, 10);
        let c = UiRect::new(20, 20, 4, 4);
        assert!(a.overlaps(b));
        assert!(!a.overlaps(c));
        assert_eq!(a.intersect(b), UiRect::new(8, 8, 2, 2));
        assert!(a.intersect(c).is_empty());
    }

    #[test]
    fn contains_excludes_the_far_edges() {
        let rect = UiRect::new(5, 5, 3, 3);
        assert!(rect.contains(5, 5));
        assert!(rect.contains(7, 7));
        // The right and bottom edges belong to the next rectangle along.
        assert!(!rect.contains(8, 7));
        assert!(!rect.contains(7, 8));
    }

    #[test]
    fn cutting_strips_leaves_the_remainder_adjacent() {
        let mut rect = UiRect::new(0, 0, 20, 10);
        let top = rect.cut_top(3);
        let bottom = rect.cut_bottom(3);
        let left = rect.cut_left(4);
        assert_eq!(top, UiRect::new(0, 0, 20, 3));
        assert_eq!(bottom, UiRect::new(0, 7, 20, 3));
        assert_eq!(left, UiRect::new(0, 3, 4, 4));
        // What is left is exactly the part none of the strips took.
        assert_eq!(rect, UiRect::new(4, 3, 16, 4));
    }

    #[test]
    fn anchoring_places_within_the_parent() {
        let parent = UiRect::new(10, 10, 100, 50);
        let size = (20, 10);
        assert_eq!(
            parent.place(size, Anchor::TopLeft, (0, 0)),
            UiRect::new(10, 10, 20, 10)
        );
        assert_eq!(
            parent.place(size, Anchor::Center, (0, 0)),
            UiRect::new(50, 30, 20, 10)
        );
        assert_eq!(
            parent.place(size, Anchor::BottomRight, (0, 0)),
            UiRect::new(90, 50, 20, 10)
        );
        // An offset nudges the anchored result without changing the anchoring.
        assert_eq!(
            parent.place(size, Anchor::Center, (2, -1)),
            UiRect::new(52, 29, 20, 10)
        );
    }

    #[test]
    fn stacking_advances_by_height_plus_gap() {
        let rect = UiRect::new(0, 0, 40, 100);
        let rows = rect.stack(3, 6, 2);
        assert_eq!(rows[0].y, 0);
        assert_eq!(rows[1].y, 8);
        assert_eq!(rows[2].y, 16);
        assert_eq!(rows[0].h, 6);
    }

    #[test]
    fn union_of_an_empty_rectangle_is_the_other_one() {
        let rect = UiRect::new(4, 4, 6, 6);
        assert_eq!(UiRect::ZERO.union(rect), rect);
        assert_eq!(rect.union(UiRect::ZERO), rect);
    }
}
