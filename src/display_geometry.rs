use std::error::Error;
use std::fmt;

use crate::window_discovery::WindowBounds;

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayGeometry {
    pub display_id: u32,
    pub bounds: WindowBounds,

    // Current display mode backing pixel dimensions.
    pub pixel_width: usize,
    pub pixel_height: usize,

    // Raw CGDisplayPixelsWide/High values for comparison.
    pub cg_pixel_width: usize,
    pub cg_pixel_height: usize,

    pub scale_x: f64,
    pub scale_y: f64,
    pub is_main: bool,
    pub is_builtin: bool,
    pub rotation_degrees: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayGeometryError {
    UnsupportedPlatform,
    CoreGraphics(i32),
    NoActiveDisplays,
    DisplayModeUnavailable(u32),
    InvalidDisplayGeometry(u32),
    DisplayListChanged,
}

impl fmt::Display for DisplayGeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                write!(f, "display geometry discovery is only supported on macOS")
            }
            Self::CoreGraphics(code) => {
                write!(f, "CoreGraphics returned error code {code}")
            }
            Self::NoActiveDisplays => {
                write!(f, "CoreGraphics returned no active displays")
            }
            Self::DisplayModeUnavailable(id) => {
                write!(f, "display {id} has no available current display mode")
            }
            Self::InvalidDisplayGeometry(id) => {
                write!(f, "display {id} returned invalid geometry")
            }
            Self::DisplayListChanged => {
                write!(f, "active display list kept changing during discovery")
            }
        }
    }
}

impl Error for DisplayGeometryError {}

/// Read the geometry of all active displays without changing display settings.
#[cfg(target_os = "macos")]
pub fn discover_displays() -> Result<Vec<DisplayGeometry>, DisplayGeometryError> {
    macos::discover_displays()
}

#[cfg(not(target_os = "macos"))]
pub fn discover_displays() -> Result<Vec<DisplayGeometry>, DisplayGeometryError> {
    Err(DisplayGeometryError::UnsupportedPlatform)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{DisplayGeometry, DisplayGeometryError};
    use crate::window_discovery::WindowBounds;
    use std::ffi::{c_int, c_void};

    type CGDirectDisplayID = u32;
    type CGError = i32;
    type CGDisplayModeRef = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGMainDisplayID() -> CGDirectDisplayID;

        fn CGGetActiveDisplayList(
            max_displays: u32,
            active_displays: *mut CGDirectDisplayID,
            display_count: *mut u32,
        ) -> CGError;

        fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;

        fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> c_int;

        fn CGDisplayRotation(display: CGDirectDisplayID) -> f64;

        fn CGDisplayPixelsWide(display: CGDirectDisplayID) -> usize;

        fn CGDisplayPixelsHigh(display: CGDirectDisplayID) -> usize;

        fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> CGDisplayModeRef;

        fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;

        fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;

        fn CGDisplayModeRelease(mode: CGDisplayModeRef);
    }

    // CGDisplayCopyDisplayMode returns an owned display-mode reference.
    // Release it exactly once when this guard leaves scope.
    struct OwnedDisplayMode(CGDisplayModeRef);

    impl Drop for OwnedDisplayMode {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CGDisplayModeRelease(self.0);
                }
            }
        }
    }

    fn active_display_ids() -> Result<Vec<CGDirectDisplayID>, DisplayGeometryError> {
        // The active display count can change while the list is being read.
        // Retry a bounded number of times rather than risking an undersized buffer.
        for _ in 0..3 {
            let mut required_count: u32 = 0;

            let count_result =
                unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut required_count) };

            if count_result != 0 {
                return Err(DisplayGeometryError::CoreGraphics(count_result));
            }

            if required_count == 0 {
                return Err(DisplayGeometryError::NoActiveDisplays);
            }

            let capacity = usize::try_from(required_count)
                .map_err(|_| DisplayGeometryError::DisplayListChanged)?;

            let mut displays = vec![0; capacity];
            let mut actual_count: u32 = 0;

            let list_result = unsafe {
                CGGetActiveDisplayList(required_count, displays.as_mut_ptr(), &mut actual_count)
            };

            if list_result != 0 {
                return Err(DisplayGeometryError::CoreGraphics(list_result));
            }

            if actual_count as usize > displays.len() {
                continue;
            }

            displays.truncate(actual_count as usize);

            if displays.is_empty() {
                return Err(DisplayGeometryError::NoActiveDisplays);
            }

            return Ok(displays);
        }

        Err(DisplayGeometryError::DisplayListChanged)
    }

    pub(super) fn discover_displays() -> Result<Vec<DisplayGeometry>, DisplayGeometryError> {
        let display_ids = active_display_ids()?;
        let main_display_id = unsafe { CGMainDisplayID() };

        let mut geometries = Vec::with_capacity(display_ids.len());

        for display_id in display_ids {
            let rect = unsafe { CGDisplayBounds(display_id) };
            let is_builtin = unsafe { CGDisplayIsBuiltin(display_id) } != 0;
            let rotation_degrees = unsafe { CGDisplayRotation(display_id) };

            // Preserve these values independently. They are not assumed to
            // represent the current HiDPI backing pixel dimensions.
            let cg_pixel_width = unsafe { CGDisplayPixelsWide(display_id) };
            let cg_pixel_height = unsafe { CGDisplayPixelsHigh(display_id) };

            let mode_ptr = unsafe { CGDisplayCopyDisplayMode(display_id) };

            if mode_ptr.is_null() {
                return Err(DisplayGeometryError::DisplayModeUnavailable(display_id));
            }

            let mode = OwnedDisplayMode(mode_ptr);

            let pixel_width = unsafe { CGDisplayModeGetPixelWidth(mode.0) };

            let pixel_height = unsafe { CGDisplayModeGetPixelHeight(mode.0) };

            // Release the mode before validating or returning.
            drop(mode);

            let logical_width = rect.size.width;
            let logical_height = rect.size.height;

            if !rect.origin.x.is_finite()
                || !rect.origin.y.is_finite()
                || !logical_width.is_finite()
                || !logical_height.is_finite()
                || !rotation_degrees.is_finite()
                || logical_width <= 0.0
                || logical_height <= 0.0
                || pixel_width == 0
                || pixel_height == 0
                || cg_pixel_width == 0
                || cg_pixel_height == 0
            {
                return Err(DisplayGeometryError::InvalidDisplayGeometry(display_id));
            }

            // Backing scale uses current display-mode pixels, not
            // CGDisplayPixelsWide/High.
            let scale_x = pixel_width as f64 / logical_width;
            let scale_y = pixel_height as f64 / logical_height;

            if !scale_x.is_finite() || !scale_y.is_finite() || scale_x <= 0.0 || scale_y <= 0.0 {
                return Err(DisplayGeometryError::InvalidDisplayGeometry(display_id));
            }

            geometries.push(DisplayGeometry {
                display_id,
                bounds: WindowBounds {
                    x: rect.origin.x,
                    y: rect.origin.y,
                    width: logical_width,
                    height: logical_height,
                },
                pixel_width,
                pixel_height,
                cg_pixel_width,
                cg_pixel_height,
                scale_x,
                scale_y,
                is_main: display_id == main_display_id,
                is_builtin,
                rotation_degrees,
            });
        }

        if !geometries.iter().any(|display| display.is_main) {
            return Err(DisplayGeometryError::InvalidDisplayGeometry(
                main_display_id,
            ));
        }

        Ok(geometries)
    }
}
