//! Mapping a pointer position inside one display onto the virtual desktop.
//!
//! WHY THIS EXISTS
//!
//! The viewer sends a position normalised to the display it is showing
//! (0.0–1.0 within that screen). Every injection backend then treated those
//! numbers as if they were normalised to the *primary* display, so on any
//! multi-monitor host the pointer landed on the wrong screen — or the wrong
//! part of the right one:
//!
//!   Windows  MOUSEEVENTF_ABSOLUTE without MOUSEEVENTF_VIRTUALDESK maps to the
//!            primary monitor, per Microsoft's MOUSEINPUT documentation.
//!   macOS    screen_point() scaled by CGDisplay::main().bounds() regardless of
//!            which display was selected.
//!   Linux    uinput ABS 0–65535 is spread by the compositor across the whole
//!            virtual desktop, which is simply a different space from
//!            "normalised within the selected display".
//!
//! Fixing any one of those alone is a regression: adding VIRTUALDESK on its own
//! moves (0.5, 0.5) from "centre of the primary screen" to "centre of the
//! virtual desktop", which is wrong for the ordinary single-monitor user. The
//! coordinate transform has to land at the same time, which is what this is.
//!
//! THE INVARIANT THAT MAKES THIS SAFE
//!
//! For a single display at the origin, both functions here are the identity —
//! `virtual_normalized` returns exactly what it was given. That is asserted by
//! test, and it is why the per-OS changes can be made without regressing the
//! common case.

use crate::message::DisplayInfo;

/// A display's placement on the virtual desktop, in pixels.
///
/// `x`/`y` may be negative: a monitor positioned to the left of, or above, the
/// primary one has a negative origin on both Windows and macOS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Bounds {
    fn right(&self) -> i64 {
        self.x as i64 + self.width as i64
    }
    fn bottom(&self) -> i64 {
        self.y as i64 + self.height as i64
    }
}

/// The rectangle enclosing every display.
///
/// Returns None when there are no displays, rather than inventing a desktop —
/// a caller with no displays has nothing sensible to do with coordinates.
pub fn virtual_desktop(displays: &[DisplayInfo]) -> Option<Bounds> {
    let first = displays.first()?;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = i64::from(first.x) + i64::from(first.width);
    let mut max_y = i64::from(first.y) + i64::from(first.height);
    for d in &displays[1..] {
        min_x = min_x.min(d.x);
        min_y = min_y.min(d.y);
        max_x = max_x.max(i64::from(d.x) + i64::from(d.width));
        max_y = max_y.max(i64::from(d.y) + i64::from(d.height));
    }
    Some(Bounds {
        x: min_x,
        y: min_y,
        // Clamped: a desktop wider than u32 cannot exist, but the arithmetic
        // above is i64 so the conversion must be explicit rather than wrapping.
        width: (max_x - i64::from(min_x)).clamp(0, u32::MAX as i64) as u32,
        height: (max_y - i64::from(min_y)).clamp(0, u32::MAX as i64) as u32,
    })
}

/// Clamp to 0.0–1.0, mapping NaN to 0.0.
///
/// `f32::clamp` PROPAGATES NaN rather than clamping it, so a plain `.clamp()`
/// would hand NaN to an injection backend and from there to the OS. Caught by
/// `out_of_range_input_is_clamped_not_wrapped`.
fn unit(v: f32) -> f64 {
    if v.is_nan() {
        return 0.0;
    }
    f64::from(v).clamp(0.0, 1.0)
}

/// Pick the requested display, falling back to the primary and then the first.
///
/// A stale index must not silently move the pointer to a different screen than
/// the one the viewer is looking at, but neither should it drop the event; the
/// primary is the least surprising target.
fn select(displays: &[DisplayInfo], index: u8) -> Option<&DisplayInfo> {
    displays
        .iter()
        .find(|d| d.index == index)
        .or_else(|| displays.iter().find(|d| d.primary))
        .or_else(|| displays.first())
}

/// Absolute pixel position on the virtual desktop.
///
/// This is what macOS wants: `CGEvent` coordinates are global, with the origin
/// at the top-left of the main display.
pub fn absolute(displays: &[DisplayInfo], index: u8, nx: f32, ny: f32) -> Option<(f64, f64)> {
    let d = select(displays, index)?;
    let (nx, ny) = (unit(nx), unit(ny));
    Some((
        f64::from(d.x) + nx * f64::from(d.width),
        f64::from(d.y) + ny * f64::from(d.height),
    ))
}

/// Position as a 0.0–1.0 fraction of the whole virtual desktop.
///
/// This is what Windows (`MOUSEEVENTF_VIRTUALDESK`, scaled to 0–65535) and
/// Linux uinput (ABS 0–65535) want.
///
/// For one display at the origin this returns its input unchanged, which is the
/// property that keeps the single-monitor case bit-identical to the old
/// behaviour.
pub fn virtual_normalized(
    displays: &[DisplayInfo],
    index: u8,
    nx: f32,
    ny: f32,
) -> Option<(f64, f64)> {
    let (ax, ay) = absolute(displays, index, nx, ny)?;
    let vd = virtual_desktop(displays)?;
    if vd.width == 0 || vd.height == 0 {
        return None;
    }
    Some((
        ((ax - f64::from(vd.x)) / f64::from(vd.width)).clamp(0.0, 1.0),
        ((ay - f64::from(vd.y)) / f64::from(vd.height)).clamp(0.0, 1.0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(index: u8, x: i32, y: i32, width: u32, height: u32, primary: bool) -> DisplayInfo {
        DisplayInfo {
            index,
            x,
            y,
            width,
            height,
            refresh_hz: 60,
            name: format!("D{index}"),
            primary,
        }
    }

    fn one() -> Vec<DisplayInfo> {
        vec![d(0, 0, 0, 1920, 1080, true)]
    }

    /// Side by side, secondary to the right: desktop is 3840x1080.
    fn side_by_side() -> Vec<DisplayInfo> {
        vec![d(0, 0, 0, 1920, 1080, true), d(1, 1920, 0, 1920, 1080, false)]
    }

    /// Secondary to the LEFT, so the primary is not at the origin and the
    /// virtual desktop starts at a negative x.
    fn left_of_primary() -> Vec<DisplayInfo> {
        vec![d(0, 0, 0, 1920, 1080, true), d(1, -1280, 0, 1280, 1024, false)]
    }

    #[test]
    fn single_display_normalisation_is_the_identity() {
        // The whole safety argument for changing the per-OS backends rests on
        // this: the ordinary one-monitor user sees no change at all.
        for (nx, ny) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (0.25, 0.75)] {
            let (vx, vy) = virtual_normalized(&one(), 0, nx, ny).unwrap();
            assert!((vx - f64::from(nx)).abs() < 1e-9, "x moved: {nx} -> {vx}");
            assert!((vy - f64::from(ny)).abs() < 1e-9, "y moved: {ny} -> {vy}");
        }
    }

    #[test]
    fn absolute_places_the_point_inside_the_selected_display() {
        let ds = side_by_side();
        assert_eq!(absolute(&ds, 0, 0.5, 0.5), Some((960.0, 540.0)));
        // Centre of the SECOND screen, not of the desktop.
        assert_eq!(absolute(&ds, 1, 0.5, 0.5), Some((2880.0, 540.0)));
    }

    #[test]
    fn secondary_display_maps_to_the_right_half_of_the_desktop() {
        let ds = side_by_side();
        let (vx, _) = virtual_normalized(&ds, 1, 0.5, 0.5).unwrap();
        assert!((vx - 0.75).abs() < 1e-9, "expected 0.75 of the desktop, got {vx}");
        // And the primary's centre is a quarter across, not a half.
        let (px, _) = virtual_normalized(&ds, 0, 0.5, 0.5).unwrap();
        assert!((px - 0.25).abs() < 1e-9, "got {px}");
    }

    #[test]
    fn handles_a_display_at_a_negative_origin() {
        let ds = left_of_primary();
        let vd = virtual_desktop(&ds).unwrap();
        assert_eq!((vd.x, vd.width), (-1280, 3200));
        // Left edge of the left-hand screen is the left edge of the desktop.
        let (vx, _) = virtual_normalized(&ds, 1, 0.0, 0.0).unwrap();
        assert!(vx.abs() < 1e-9, "got {vx}");
        // Right edge of the primary is the right edge of the desktop.
        let (vx, _) = virtual_normalized(&ds, 0, 1.0, 0.0).unwrap();
        assert!((vx - 1.0).abs() < 1e-9, "got {vx}");
    }

    #[test]
    fn desktop_of_differing_heights_uses_the_tallest() {
        let ds = left_of_primary(); // 1080 and 1024
        let vd = virtual_desktop(&ds).unwrap();
        assert_eq!((vd.y, vd.height), (0, 1080));
    }

    #[test]
    fn an_unknown_index_falls_back_to_primary_rather_than_dropping_the_event() {
        let ds = side_by_side();
        assert_eq!(absolute(&ds, 9, 0.5, 0.5), absolute(&ds, 0, 0.5, 0.5));
    }

    #[test]
    fn out_of_range_input_is_clamped_not_wrapped() {
        let ds = one();
        assert_eq!(virtual_normalized(&ds, 0, -5.0, 5.0), Some((0.0, 1.0)));
        let (nan_x, _) = virtual_normalized(&ds, 0, f32::NAN, 0.5).unwrap();
        assert!(nan_x.is_finite(), "NaN must not reach an injection backend");
    }

    #[test]
    fn no_displays_yields_nothing_rather_than_a_guess() {
        assert_eq!(virtual_desktop(&[]), None);
        assert_eq!(absolute(&[], 0, 0.5, 0.5), None);
        assert_eq!(virtual_normalized(&[], 0, 0.5, 0.5), None);
    }

    #[test]
    fn a_zero_sized_desktop_does_not_divide_by_zero() {
        let ds = vec![d(0, 0, 0, 0, 0, true)];
        assert_eq!(virtual_normalized(&ds, 0, 0.5, 0.5), None);
    }
}
