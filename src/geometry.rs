//! Page geometry types: the display-space box and rotation of each page.
//!
//! The PDF layer passed page geometry around as parallel vectors and
//! bare primitives, with the invariants (equal lengths, normalized
//! corners, rotation limited to quarter turns) living only in comments.
//! These types move them into the compiler: one [`PageGeometry`] per
//! page, parsed once at open.

use lopdf::ObjectId;

/// Clockwise `/Rotate` in quarter turns, parsed once via
/// [`Rotation::from_degrees`]. Values that are not a multiple of 90
/// are invalid per the PDF spec and map to [`Rotation::R0`] there, so
/// no other code re-derives the fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    /// Normalize `degrees` (any integer, any sign) to quarter turns.
    #[must_use]
    pub fn from_degrees(degrees: i64) -> Self {
        match degrees.rem_euclid(360) {
            90 => Self::R90,
            180 => Self::R180,
            270 => Self::R270,
            _ => Self::R0,
        }
    }

    /// Whether the display size swaps width and height.
    #[must_use]
    pub fn is_sideways(self) -> bool {
        matches!(self, Self::R90 | Self::R270)
    }
}

/// Effective visible box (`CropBox` ∩ `MediaBox`) in unrotated
/// user-space points. Corners are normalized on construction (ISO
/// 32000-1 §7.9.5 allows the two diagonally opposite corners in any
/// order), so there is no `(x, y, w, h)` vs `(x0, y0, x1, y1)`
/// ambiguity: the fields are always ordered minima and maxima.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl PageRect {
    /// Ordered-corner rectangle from any two opposite corners.
    #[must_use]
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }

    /// Width in points (`x1 - x0`).
    #[must_use]
    pub fn width(self) -> f32 {
        self.x1 - self.x0
    }

    /// Height in points (`y1 - y0`).
    #[must_use]
    pub fn height(self) -> f32 {
        self.y1 - self.y0
    }
}

/// One page's identity and display geometry.
#[derive(Debug, Clone, Copy)]
pub struct PageGeometry {
    /// Object id of the page dictionary.
    pub id: ObjectId,
    /// Effective visible box in unrotated user-space points.
    pub rect: PageRect,
    /// Clockwise display rotation.
    pub rotation: Rotation,
}

impl PageGeometry {
    /// Display size in points (rotation-aware): 90°/270° swap axes.
    #[must_use]
    pub fn display_size(&self) -> (f32, f32) {
        let (w, h) = (self.rect.width(), self.rect.height());
        if self.rotation.is_sideways() {
            (h, w)
        } else {
            (w, h)
        }
    }
}
