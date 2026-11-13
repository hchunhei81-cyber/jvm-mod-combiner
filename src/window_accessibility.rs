use crate::window_discovery::{DiscoveredWindow, WindowBounds};
use std::error::Error;
use std::ffi::{CStr, c_char, c_void};
use std::fmt;
use std::ptr;
use std::ptr::NonNull;

#[derive(Debug, Clone, PartialEq)]
pub struct AccessibilityWindow {
    pub index: usize,
    pub title: Option<String>,
    pub bounds: Option<WindowBounds>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibilityError {
    UnsupportedPlatform,
    PermissionDenied,
    InvalidPid,
    ApplicationUnavailable,
    AxCall {
        operation: &'static str,
        code: i32,
    },
    InvalidReturnedType(&'static str),
    WindowIdentityUnavailable,
    WindowIdentityChanged(&'static str),
    WindowDiscoveryFailed,
    WindowNotFound,
    AmbiguousWindowMatch(usize),
    InvalidWindowBounds,
    ProcessMismatch {
        expected: u32,
        actual: u32,
    },
    AttributeNotSettable(&'static str),
    ValueCreationFailed(&'static str),
    VerificationFailed,
    VerificationMismatch {
        actual: String,
        expected: String,
    },
    RollbackFailed {
        operation: Box<AccessibilityError>,
        rollback: &'static str,
    },
}

impl fmt::Display for AccessibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                write!(f, "Accessibility API is only available on macOS")
            }
            Self::PermissionDenied => {
                write!(f, "Accessibility permission is not granted")
            }
            Self::InvalidPid => write!(f, "PID must be a positive valid process ID"),
            Self::ApplicationUnavailable => {
                write!(f, "Could not create an Accessibility application element")
            }
            Self::AxCall { operation, code } => {
                write!(f, "{operation} failed with AXError {code}")
            }
            Self::InvalidReturnedType(attribute) => {
                write!(f, "Unexpected CoreFoundation type for {attribute}")
            }
            Self::WindowIdentityUnavailable => {
                write!(
                    f,
                    "Window title or bounds are unavailable for safe matching"
                )
            }
            Self::WindowIdentityChanged(field) => {
                write!(f, "Window identity changed before modification: {field}")
            }
            Self::WindowDiscoveryFailed => {
                write!(f, "Could not re-discover windows before modification")
            }
            Self::WindowNotFound => {
                write!(f, "No Accessibility window matches the discovered window")
            }
            Self::AmbiguousWindowMatch(count) => {
                write!(
                    f,
                    "Window identity is ambiguous: {count} Accessibility windows match"
                )
            }
            Self::InvalidWindowBounds => {
                write!(f, "Window bounds contain invalid or non-positive geometry")
            }
            Self::ProcessMismatch { expected, actual } => {
                write!(f, "Window PID mismatch: expected {expected}, got {actual}")
            }
            Self::AttributeNotSettable(attribute) => {
                write!(f, "Accessibility attribute {attribute} is not settable")
            }
            Self::ValueCreationFailed(operation) => {
                write!(f, "Could not create Accessibility value for {operation}")
            }
            Self::VerificationFailed => {
                write!(
                    f,
                    "Window geometry did not match the requested bounds after update"
                )
            }
            Self::VerificationMismatch { actual, expected } => {
                write!(
                    f,
                    "Window geometry verification failed: {actual}; expected {expected}"
                )
            }
            Self::RollbackFailed {
                operation,
                rollback,
            } => {
                write!(
                    f,
                    "Window operation failed: {operation}; rollback also failed: {rollback}"
                )
            }
        }
    }
}

impl Error for AccessibilityError {}

/// Maximum difference, in logical screen points, accepted when matching
/// CoreGraphics geometry against Accessibility geometry.
const WINDOW_MATCH_TOLERANCE: f64 = 2.0;

fn valid_window_bounds(bounds: &WindowBounds) -> bool {
    [bounds.x, bounds.y, bounds.width, bounds.height]
        .iter()
        .all(|value| value.is_finite())
        && bounds.width > 0.0
        && bounds.height > 0.0
}

fn bounds_match(expected: &WindowBounds, actual: &WindowBounds) -> bool {
    valid_window_bounds(expected)
        && valid_window_bounds(actual)
        && (expected.x - actual.x).abs() <= WINDOW_MATCH_TOLERANCE
        && (expected.y - actual.y).abs() <= WINDOW_MATCH_TOLERANCE
        && (expected.width - actual.width).abs() <= WINDOW_MATCH_TOLERANCE
        && (expected.height - actual.height).abs() <= WINDOW_MATCH_TOLERANCE
}

/// Revalidate stable window identity without requiring unchanged geometry.
///
/// Rollback must use this check because a failed position/size write can
/// leave the window at an intermediate geometry.
#[cfg(any(target_os = "macos", test))]
fn verify_fresh_window_identity_only(
    expected: &DiscoveredWindow,
    current_windows: &[DiscoveredWindow],
) -> Result<(), AccessibilityError> {
    let matches: Vec<&DiscoveredWindow> = current_windows
        .iter()
        .filter(|window| window.window_id == expected.window_id)
        .collect();

    if matches.is_empty() {
        return Err(AccessibilityError::WindowNotFound);
    }

    if matches.len() != 1 {
        return Err(AccessibilityError::AmbiguousWindowMatch(matches.len()));
    }

    let actual = matches[0];

    if actual.pid != expected.pid {
        return Err(AccessibilityError::ProcessMismatch {
            expected: expected.pid,
            actual: actual.pid,
        });
    }

    if actual.owner_name != expected.owner_name {
        return Err(AccessibilityError::WindowIdentityChanged("owner"));
    }

    if actual.title != expected.title {
        return Err(AccessibilityError::WindowIdentityChanged("title"));
    }

    Ok(())
}

/// Revalidate a previously discovered window against a fresh
/// CoreGraphics snapshot. Window IDs must be unique in the fresh snapshot.
/// This variant also requires geometry to remain unchanged.
#[cfg(any(target_os = "macos", test))]
fn verify_fresh_window_identity(
    expected: &DiscoveredWindow,
    current_windows: &[DiscoveredWindow],
) -> Result<(), AccessibilityError> {
    verify_fresh_window_identity_only(expected, current_windows)?;

    let actual = current_windows
        .iter()
        .find(|window| window.window_id == expected.window_id)
        .ok_or(AccessibilityError::WindowNotFound)?;

    if !bounds_match(&expected.bounds, &actual.bounds) {
        return Err(AccessibilityError::WindowIdentityChanged("geometry"));
    }

    Ok(())
}

/// Verify that a failed window operation was rolled back to its original
/// geometry. Accessibility read-back can differ slightly, so use a
/// one-logical-point tolerance.
#[cfg(any(target_os = "macos", test))]
fn geometry_matches(actual: &WindowBounds, expected: &WindowBounds) -> bool {
    const VERIFY_TOLERANCE: f64 = 1.0;

    valid_window_bounds(actual)
        && valid_window_bounds(expected)
        && (actual.x - expected.x).abs() <= VERIFY_TOLERANCE
        && (actual.y - expected.y).abs() <= VERIFY_TOLERANCE
        && (actual.width - expected.width).abs() <= VERIFY_TOLERANCE
        && (actual.height - expected.height).abs() <= VERIFY_TOLERANCE
}

/// Execute the window geometry mutation as one recoverable transaction.
///
/// Every operation failure triggers rollback, including a failed first
/// setter, because a failed system call does not prove that no mutation
/// occurred.
#[cfg(any(target_os = "macos", test))]
fn run_window_transaction(
    mut set_position: impl FnMut() -> Result<(), AccessibilityError>,
    mut set_size: impl FnMut() -> Result<(), AccessibilityError>,
    mut verify: impl FnMut() -> Result<(), AccessibilityError>,
    mut rollback: impl FnMut() -> Result<(), &'static str>,
) -> Result<(), AccessibilityError> {
    let operation = (|| {
        set_position()?;
        set_size()?;
        verify()
    })();

    match operation {
        Ok(()) => Ok(()),
        Err(error) => match rollback() {
            Ok(()) => Err(error),
            Err(rollback) => Err(AccessibilityError::RollbackFailed {
                operation: Box::new(error),
                rollback,
            }),
        },
    }
}

/// Match a window during rollback using its unique title within the
/// already-scoped process AXWindows collection.
///
/// Unlike normal forward matching, rollback cannot require AX geometry to
/// match CoreGraphics geometry: a failed setter may have partially changed
/// the window. The caller must independently revalidate the CoreGraphics
/// Window ID, PID, owner and title before using the returned transient index.
/// Duplicate titles and invalid AX geometry are still rejected.
fn match_accessibility_window_for_rollback(
    expected_title: Option<&str>,
    candidates: &[AccessibilityWindow],
) -> Result<usize, AccessibilityError> {
    let title = expected_title
        .filter(|title| !title.is_empty())
        .ok_or(AccessibilityError::WindowIdentityUnavailable)?;

    let matches: Vec<&AccessibilityWindow> = candidates
        .iter()
        .filter(|candidate| candidate.title.as_deref() == Some(title))
        .collect();

    match matches.as_slice() {
        [] => Err(AccessibilityError::WindowNotFound),
        [candidate] => {
            let bounds = candidate
                .bounds
                .as_ref()
                .ok_or(AccessibilityError::InvalidWindowBounds)?;

            if !valid_window_bounds(bounds) {
                return Err(AccessibilityError::InvalidWindowBounds);
            }

            Ok(candidate.index)
        }
        _ => Err(AccessibilityError::AmbiguousWindowMatch(matches.len())),
    }
}

/// Match a CoreGraphics-discovered window to the current Accessibility
/// enumeration for the same process.
///
/// The returned index is transient and must never be persisted across
/// Accessibility enumerations. Matching requires an exact, non-empty title
/// and geometry within the configured logical-point tolerance.
///
/// Ambiguous matches are rejected rather than guessed.
pub fn match_accessibility_window(
    discovered: &DiscoveredWindow,
    candidates: &[AccessibilityWindow],
) -> Result<usize, AccessibilityError> {
    if discovered.title.as_deref().is_none_or(str::is_empty)
        || !valid_window_bounds(&discovered.bounds)
    {
        return Err(AccessibilityError::WindowIdentityUnavailable);
    }

    let title = discovered.title.as_deref().unwrap();

    let matches: Vec<usize> = candidates
        .iter()
        .filter(|candidate| {
            candidate.title.as_deref() == Some(title)
                && candidate
                    .bounds
                    .as_ref()
                    .is_some_and(|bounds| bounds_match(&discovered.bounds, bounds))
        })
        .map(|candidate| candidate.index)
        .collect();

    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(AccessibilityError::WindowNotFound),
        _ => Err(AccessibilityError::AmbiguousWindowMatch(matches.len())),
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::mem::MaybeUninit;

    type CFIndex = isize;
    type CFTypeID = usize;
    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFArrayRef = *const c_void;
    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    const K_AX_VALUE_TYPE_CG_POINT: u32 = 1;
    const K_AX_VALUE_TYPE_CG_SIZE: u32 = 2;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;

        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;

        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementIsAttributeSettable(
            element: AXUIElementRef,
            attribute: CFStringRef,
            settable: *mut u8,
        ) -> i32;
        fn AXUIElementSetAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> i32;
        fn AXValueCreate(value_type: u32, value: *const c_void) -> AXValueRef;

        fn AXValueGetType(value: AXValueRef) -> u32;

        fn AXValueGetTypeID() -> CFTypeID;

        fn AXValueGetValue(value: AXValueRef, value_type: u32, output: *mut c_void) -> u8;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: CFTypeRef);

        fn CFGetTypeID(value: CFTypeRef) -> CFTypeID;

        fn CFArrayGetTypeID() -> CFTypeID;

        fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;

        fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;

        fn CFStringCreateWithCString(
            allocator: *const c_void,
            string: *const c_char,
            encoding: u32,
        ) -> CFStringRef;

        fn CFStringGetTypeID() -> CFTypeID;

        fn CFStringGetLength(string: CFStringRef) -> CFIndex;

        fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;

        fn CFStringGetCString(
            string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> u8;
    }

    struct OwnedCf(NonNull<c_void>);

    impl OwnedCf {
        unsafe fn from_raw(value: CFTypeRef) -> Option<Self> {
            NonNull::new(value as *mut c_void).map(Self)
        }

        fn as_ptr(&self) -> CFTypeRef {
            self.0.as_ptr()
        }
    }

    impl Drop for OwnedCf {
        fn drop(&mut self) {
            unsafe {
                CFRelease(self.as_ptr());
            }
        }
    }

    fn create_attribute(name: &'static [u8]) -> Option<OwnedCf> {
        let value = unsafe {
            CFStringCreateWithCString(
                ptr::null(),
                name.as_ptr() as *const c_char,
                K_CF_STRING_ENCODING_UTF8,
            )
        };

        unsafe { OwnedCf::from_raw(value) }
    }

    fn copy_attribute(
        element: AXUIElementRef,
        attribute: &OwnedCf,
        operation: &'static str,
    ) -> Result<OwnedCf, AccessibilityError> {
        let mut value: CFTypeRef = ptr::null();

        let result =
            unsafe { AXUIElementCopyAttributeValue(element, attribute.as_ptr(), &mut value) };

        if result != 0 {
            if !value.is_null() {
                unsafe {
                    CFRelease(value);
                }
            }

            return Err(AccessibilityError::AxCall {
                operation,
                code: result,
            });
        }

        unsafe {
            OwnedCf::from_raw(value).ok_or(AccessibilityError::InvalidReturnedType(operation))
        }
    }

    fn cf_string_to_string(value: CFTypeRef) -> Option<String> {
        unsafe {
            if CFGetTypeID(value) != CFStringGetTypeID() {
                return None;
            }

            let length = CFStringGetLength(value);
            let capacity = CFStringGetMaximumSizeForEncoding(length, K_CF_STRING_ENCODING_UTF8);

            if capacity < 0 {
                return None;
            }

            let buffer_size = usize::try_from(capacity)
                .ok()
                .and_then(|size| size.checked_add(1))
                .and_then(|size| CFIndex::try_from(size).ok().map(|cf_size| (size, cf_size)));

            let Some((buffer_size, cf_buffer_size)) = buffer_size else {
                return None;
            };

            let mut buffer = vec![0u8; buffer_size];

            let success = CFStringGetCString(
                value,
                buffer.as_mut_ptr() as *mut c_char,
                cf_buffer_size,
                K_CF_STRING_ENCODING_UTF8,
            );

            if success == 0 {
                return None;
            }

            CStr::from_ptr(buffer.as_ptr() as *const c_char)
                .to_str()
                .ok()
                .map(str::to_owned)
        }
    }

    fn read_point(value: CFTypeRef) -> Option<CGPoint> {
        unsafe {
            if CFGetTypeID(value) != AXValueGetTypeID()
                || AXValueGetType(value) != K_AX_VALUE_TYPE_CG_POINT
            {
                return None;
            }

            let mut point = MaybeUninit::<CGPoint>::uninit();

            if AXValueGetValue(
                value,
                K_AX_VALUE_TYPE_CG_POINT,
                point.as_mut_ptr() as *mut c_void,
            ) == 0
            {
                return None;
            }

            Some(point.assume_init())
        }
    }

    fn read_size(value: CFTypeRef) -> Option<CGSize> {
        unsafe {
            if CFGetTypeID(value) != AXValueGetTypeID()
                || AXValueGetType(value) != K_AX_VALUE_TYPE_CG_SIZE
            {
                return None;
            }

            let mut size = MaybeUninit::<CGSize>::uninit();

            if AXValueGetValue(
                value,
                K_AX_VALUE_TYPE_CG_SIZE,
                size.as_mut_ptr() as *mut c_void,
            ) == 0
            {
                return None;
            }

            Some(size.assume_init())
        }
    }

    fn attribute_is_settable(
        element: AXUIElementRef,
        attribute: &OwnedCf,
        name: &'static str,
    ) -> Result<(), AccessibilityError> {
        let mut settable = 0u8;
        let result =
            unsafe { AXUIElementIsAttributeSettable(element, attribute.as_ptr(), &mut settable) };

        if result != 0 {
            return Err(AccessibilityError::AxCall {
                operation: name,
                code: result,
            });
        }

        if settable == 0 {
            return Err(AccessibilityError::AttributeNotSettable(name));
        }

        Ok(())
    }

    fn set_ax_value(
        element: AXUIElementRef,
        attribute: &OwnedCf,
        value_type: u32,
        value: *const c_void,
        operation: &'static str,
    ) -> Result<(), AccessibilityError> {
        let value = unsafe { OwnedCf::from_raw(AXValueCreate(value_type, value)) }
            .ok_or(AccessibilityError::ValueCreationFailed(operation))?;

        let result =
            unsafe { AXUIElementSetAttributeValue(element, attribute.as_ptr(), value.as_ptr()) };

        if result != 0 {
            return Err(AccessibilityError::AxCall {
                operation,
                code: result,
            });
        }

        Ok(())
    }

    fn set_point(
        element: AXUIElementRef,
        attribute: &OwnedCf,
        point: CGPoint,
    ) -> Result<(), AccessibilityError> {
        set_ax_value(
            element,
            attribute,
            K_AX_VALUE_TYPE_CG_POINT,
            &point as *const CGPoint as *const c_void,
            "AXPosition",
        )
    }

    fn set_size(
        element: AXUIElementRef,
        attribute: &OwnedCf,
        size: CGSize,
    ) -> Result<(), AccessibilityError> {
        set_ax_value(
            element,
            attribute,
            K_AX_VALUE_TYPE_CG_SIZE,
            &size as *const CGSize as *const c_void,
            "AXSize",
        )
    }

    fn restore_bounds(
        element: AXUIElementRef,
        position_attribute: &OwnedCf,
        size_attribute: &OwnedCf,
        old_position: CGPoint,
        old_size: CGSize,
    ) -> Result<(), &'static str> {
        // Attempt both restorations independently. A failure to restore
        // size must not prevent us from attempting to restore position.
        let size_result = set_size(element, size_attribute, old_size);
        let position_result = set_point(element, position_attribute, old_position);

        match (size_result.is_err(), position_result.is_err()) {
            (true, true) => return Err("restoring AXSize and AXPosition"),
            (true, false) => return Err("restoring AXSize"),
            (false, true) => return Err("restoring AXPosition"),
            (false, false) => {}
        }

        // Setter success is not proof that macOS applied the requested
        // geometry. Read both attributes back before reporting rollback
        // success.
        let restored_position =
            copy_attribute(element, position_attribute, "verifying restored AXPosition")
                .map_err(|_| "reading restored AXPosition")?;

        let restored_position =
            read_point(restored_position.as_ptr()).ok_or("decoding restored AXPosition")?;

        let restored_size = copy_attribute(element, size_attribute, "verifying restored AXSize")
            .map_err(|_| "reading restored AXSize")?;

        let restored_size = read_size(restored_size.as_ptr()).ok_or("decoding restored AXSize")?;

        let actual = WindowBounds {
            x: restored_position.x,
            y: restored_position.y,
            width: restored_size.width,
            height: restored_size.height,
        };

        let expected = WindowBounds {
            x: old_position.x,
            y: old_position.y,
            width: old_size.width,
            height: old_size.height,
        };

        if !geometry_matches(&actual, &expected) {
            return Err("verifying restored geometry");
        }

        Ok(())
    }

    /// Reacquire the window through fresh CoreGraphics and Accessibility
    /// snapshots before restoring its original geometry. Never reuse the
    /// AXUIElementRef selected by the failed forward operation.
    fn restore_bounds_safely(
        pid: u32,
        expected: &DiscoveredWindow,
        old_position: CGPoint,
        old_size: CGSize,
    ) -> Result<(), &'static str> {
        if pid == 0 || pid != expected.pid || pid > i32::MAX as u32 {
            return Err("validating rollback PID");
        }

        let current_windows = crate::window_discovery::discover_windows()
            .map_err(|_| "rediscovering window before rollback")?;

        verify_fresh_window_identity_only(expected, &current_windows)
            .map_err(|_| "revalidating window identity before rollback")?;

        let fresh_window = current_windows
            .iter()
            .find(|window| window.window_id == expected.window_id)
            .ok_or("finding window before rollback")?;

        let application = unsafe { OwnedCf::from_raw(AXUIElementCreateApplication(pid as i32)) }
            .ok_or("creating Accessibility application for rollback")?;

        let windows_attribute =
            create_attribute(b"AXWindows\0").ok_or("creating AXWindows attribute")?;
        let title_attribute = create_attribute(b"AXTitle\0").ok_or("creating AXTitle attribute")?;
        let position_attribute =
            create_attribute(b"AXPosition\0").ok_or("creating AXPosition attribute")?;
        let size_attribute = create_attribute(b"AXSize\0").ok_or("creating AXSize attribute")?;

        // Keep this fresh array alive for the complete rollback operation.
        let windows = copy_attribute(
            application.as_ptr(),
            &windows_attribute,
            "AXWindows before rollback",
        )
        .map_err(|_| "reading AXWindows before rollback")?;

        unsafe {
            if CFGetTypeID(windows.as_ptr()) != CFArrayGetTypeID() {
                return Err("validating AXWindows before rollback");
            }
        }

        let count = unsafe { CFArrayGetCount(windows.as_ptr()) };
        if count < 0 {
            return Err("counting AXWindows before rollback");
        }

        let mut candidates = Vec::with_capacity(count as usize);
        let mut rollback_candidate_diagnostics = Vec::with_capacity(count as usize);

        for index in 0..count {
            let element =
                unsafe { CFArrayGetValueAtIndex(windows.as_ptr(), index) as AXUIElementRef };

            if element.is_null() {
                rollback_candidate_diagnostics.push(format!("index={index} element=null"));
                continue;
            }

            let title_result = copy_attribute(element, &title_attribute, "AXTitle before rollback");
            let (title, title_diagnostic) = match title_result {
                Ok(value) => match cf_string_to_string(value.as_ptr()) {
                    Some(title) => {
                        let diagnostic = format!("title={title:?}");
                        (Some(title), diagnostic)
                    }
                    None => (None, "title_decode_failed".to_owned()),
                },
                Err(error) => (None, format!("title_read_error={error}")),
            };

            let position_result =
                copy_attribute(element, &position_attribute, "AXPosition before rollback");
            let (position, position_diagnostic) = match position_result {
                Ok(value) => match read_point(value.as_ptr()) {
                    Some(position) => (
                        Some(position),
                        format!("position=({:.1},{:.1})", position.x, position.y),
                    ),
                    None => (None, "position_decode_failed".to_owned()),
                },
                Err(error) => (None, format!("position_read_error={error}")),
            };

            let size_result = copy_attribute(element, &size_attribute, "AXSize before rollback");
            let (size, size_diagnostic) = match size_result {
                Ok(value) => match read_size(value.as_ptr()) {
                    Some(size) => (
                        Some(size),
                        format!("size=({:.1},{:.1})", size.width, size.height),
                    ),
                    None => (None, "size_decode_failed".to_owned()),
                },
                Err(error) => (None, format!("size_read_error={error}")),
            };

            let bounds = match (position, size) {
                (Some(position), Some(size)) => Some(WindowBounds {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }),
                _ => None,
            };

            rollback_candidate_diagnostics.push(format!(
                "index={} {} {} {}",
                index, title_diagnostic, position_diagnostic, size_diagnostic
            ));

            candidates.push(AccessibilityWindow {
                index: index as usize,
                title,
                bounds,
            });
        }

        let selected_index = match_accessibility_window_for_rollback(
            fresh_window.title.as_deref(),
            &candidates,
        )
        .map_err(|error| {
            eprintln!(
                "[RollbackDiagnostic] expected_title={:?} AX_count={} candidate_count={} match_error={}",
                fresh_window.title,
                count,
                candidates.len(),
                error
            );

            for diagnostic in &rollback_candidate_diagnostics {
                eprintln!("[RollbackDiagnostic] {diagnostic}");
            }

            match error {
                AccessibilityError::WindowNotFound => {
                    "rollback AX matching failed: no candidate has the expected title"
                }
                AccessibilityError::AmbiguousWindowMatch(_) => {
                    "rollback AX matching failed: multiple candidates have the expected title"
                }
                AccessibilityError::InvalidWindowBounds => {
                    "rollback AX matching failed: matching candidate has missing or invalid bounds"
                }
                AccessibilityError::WindowIdentityUnavailable => {
                    "rollback AX matching failed: expected title is missing or empty"
                }
                _ => "rollback AX matching failed: unexpected matching error",
            }
        })?;

        if selected_index >= count as usize {
            return Err("validating rollback Accessibility index");
        }

        let element = unsafe {
            CFArrayGetValueAtIndex(windows.as_ptr(), selected_index as CFIndex) as AXUIElementRef
        };

        if element.is_null() {
            return Err("getting fresh Accessibility element for rollback");
        }

        // Revalidate stable identity once more immediately before restoration.
        let latest_windows = crate::window_discovery::discover_windows()
            .map_err(|_| "rechecking window identity before rollback")?;

        verify_fresh_window_identity_only(expected, &latest_windows)
            .map_err(|_| "rechecking window identity before rollback")?;

        let latest_window = latest_windows
            .iter()
            .find(|window| window.window_id == expected.window_id)
            .ok_or("finding latest window before rollback")?;

        let current_title_value = copy_attribute(
            element,
            &title_attribute,
            "rechecking AXTitle before rollback",
        )
        .map_err(|_| "reading AXTitle before rollback")?;

        let current_title = cf_string_to_string(current_title_value.as_ptr())
            .ok_or("decoding AXTitle before rollback")?;

        if Some(current_title.as_str()) != latest_window.title.as_deref() {
            return Err("revalidating Accessibility title before rollback");
        }

        let current_position_value = copy_attribute(
            element,
            &position_attribute,
            "rechecking AXPosition before rollback",
        )
        .map_err(|_| "reading AXPosition before rollback")?;

        let current_size_value = copy_attribute(
            element,
            &size_attribute,
            "rechecking AXSize before rollback",
        )
        .map_err(|_| "reading AXSize before rollback")?;

        let current_position = read_point(current_position_value.as_ptr())
            .ok_or("decoding AXPosition before rollback")?;
        let current_size =
            read_size(current_size_value.as_ptr()).ok_or("decoding AXSize before rollback")?;

        let current_bounds = WindowBounds {
            x: current_position.x,
            y: current_position.y,
            width: current_size.width,
            height: current_size.height,
        };

        // During rollback, AX and CoreGraphics may temporarily report
        // different geometries after a partially applied setter. Identity
        // has been revalidated independently; require readable, valid AX
        // geometry, but do not require it to match the CG snapshot here.
        if !valid_window_bounds(&current_bounds) {
            return Err("validating Accessibility geometry before rollback");
        }

        attribute_is_settable(element, &position_attribute, "AXPosition")
            .map_err(|_| "checking AXPosition settable before rollback")?;
        attribute_is_settable(element, &size_attribute, "AXSize")
            .map_err(|_| "checking AXSize settable before rollback")?;

        restore_bounds(
            element,
            &position_attribute,
            &size_attribute,
            old_position,
            old_size,
        )?;

        // Verify both stable identity and restored CoreGraphics geometry.
        let restored_windows = crate::window_discovery::discover_windows()
            .map_err(|_| "verifying window after rollback")?;

        verify_fresh_window_identity_only(expected, &restored_windows)
            .map_err(|_| "verifying window identity after rollback")?;

        let restored_window = restored_windows
            .iter()
            .find(|window| window.window_id == expected.window_id)
            .ok_or("finding window after rollback")?;

        let original_bounds = WindowBounds {
            x: old_position.x,
            y: old_position.y,
            width: old_size.width,
            height: old_size.height,
        };

        if !bounds_match(&original_bounds, &restored_window.bounds) {
            return Err("verifying restored CoreGraphics geometry");
        }

        Ok(())
    }

    pub fn set_window_bounds(
        pid: u32,
        discovered: &DiscoveredWindow,
        desired: &WindowBounds,
    ) -> Result<(), AccessibilityError> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(AccessibilityError::InvalidPid);
        }

        if discovered.pid != pid {
            return Err(AccessibilityError::ProcessMismatch {
                expected: discovered.pid,
                actual: pid,
            });
        }

        if !valid_window_bounds(desired) {
            return Err(AccessibilityError::InvalidWindowBounds);
        }

        if !is_trusted() {
            return Err(AccessibilityError::PermissionDenied);
        }

        let application = unsafe { OwnedCf::from_raw(AXUIElementCreateApplication(pid as i32)) }
            .ok_or(AccessibilityError::ApplicationUnavailable)?;

        let windows_attribute =
            create_attribute(b"AXWindows\0").ok_or(AccessibilityError::ApplicationUnavailable)?;
        let title_attribute =
            create_attribute(b"AXTitle\0").ok_or(AccessibilityError::ApplicationUnavailable)?;
        let position_attribute =
            create_attribute(b"AXPosition\0").ok_or(AccessibilityError::ApplicationUnavailable)?;
        let size_attribute =
            create_attribute(b"AXSize\0").ok_or(AccessibilityError::ApplicationUnavailable)?;

        // Keep this AXWindows array alive until all operations on the
        // selected borrowed AXUIElement have completed.
        let windows = copy_attribute(application.as_ptr(), &windows_attribute, "AXWindows")?;

        unsafe {
            if CFGetTypeID(windows.as_ptr()) != CFArrayGetTypeID() {
                return Err(AccessibilityError::InvalidReturnedType("AXWindows"));
            }
        }

        let count = unsafe { CFArrayGetCount(windows.as_ptr()) };
        if count < 0 {
            return Err(AccessibilityError::InvalidReturnedType("AXWindows"));
        }

        let mut candidates = Vec::with_capacity(count as usize);

        for index in 0..count {
            let element =
                unsafe { CFArrayGetValueAtIndex(windows.as_ptr(), index) as AXUIElementRef };

            if element.is_null() {
                continue;
            }

            let title = copy_attribute(element, &title_attribute, "AXTitle")
                .ok()
                .and_then(|value| cf_string_to_string(value.as_ptr()));

            let position = copy_attribute(element, &position_attribute, "AXPosition")
                .ok()
                .and_then(|value| read_point(value.as_ptr()));

            let size = copy_attribute(element, &size_attribute, "AXSize")
                .ok()
                .and_then(|value| read_size(value.as_ptr()));

            let bounds = match (position, size) {
                (Some(position), Some(size)) => Some(WindowBounds {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }),
                _ => None,
            };

            candidates.push(AccessibilityWindow {
                index: index as usize,
                title,
                bounds,
            });
        }

        let selected_index = match_accessibility_window(discovered, &candidates)?;

        if selected_index >= count as usize {
            return Err(AccessibilityError::WindowNotFound);
        }

        let element = unsafe {
            CFArrayGetValueAtIndex(windows.as_ptr(), selected_index as CFIndex) as AXUIElementRef
        };

        if element.is_null() {
            return Err(AccessibilityError::WindowNotFound);
        }

        // Check both attributes before changing either one.
        attribute_is_settable(element, &position_attribute, "AXPosition")?;
        attribute_is_settable(element, &size_attribute, "AXSize")?;

        let new_position = CGPoint {
            x: desired.x,
            y: desired.y,
        };
        let new_size = CGSize {
            width: desired.width,
            height: desired.height,
        };

        // Re-discover immediately before the first write. This narrows the
        // race between initial discovery and Accessibility mutation.
        let current_windows = crate::window_discovery::discover_windows()
            .map_err(|_| AccessibilityError::WindowDiscoveryFailed)?;
        verify_fresh_window_identity(discovered, &current_windows)?;

        // CoreGraphics validates the Window ID/PID/owner. Re-read the same
        // selected AX element as a second, independent identity check.
        let current_title_value = copy_attribute(element, &title_attribute, "rechecking AXTitle")?;
        let current_title = cf_string_to_string(current_title_value.as_ptr())
            .ok_or(AccessibilityError::WindowIdentityUnavailable)?;

        if Some(current_title.as_str()) != discovered.title.as_deref() {
            return Err(AccessibilityError::WindowIdentityChanged(
                "Accessibility title",
            ));
        }

        let current_position_value =
            copy_attribute(element, &position_attribute, "rechecking AXPosition")?;
        let current_size_value = copy_attribute(element, &size_attribute, "rechecking AXSize")?;

        let current_position = read_point(current_position_value.as_ptr())
            .ok_or(AccessibilityError::InvalidReturnedType("AXPosition"))?;
        let current_size = read_size(current_size_value.as_ptr())
            .ok_or(AccessibilityError::InvalidReturnedType("AXSize"))?;

        let current_bounds = WindowBounds {
            x: current_position.x,
            y: current_position.y,
            width: current_size.width,
            height: current_size.height,
        };

        if !bounds_match(&discovered.bounds, &current_bounds) {
            return Err(AccessibilityError::WindowIdentityChanged(
                "Accessibility geometry",
            ));
        }

        // Capture the final verified geometry as the rollback baseline.
        let old_position = current_position;
        let old_size = current_size;

        run_window_transaction(
            || set_point(element, &position_attribute, new_position),
            || set_size(element, &size_attribute, new_size),
            || {
                let position_value = copy_attribute(element, &position_attribute, "AXPosition")?;
                let size_value = copy_attribute(element, &size_attribute, "AXSize")?;

                let position = read_point(position_value.as_ptr())
                    .ok_or(AccessibilityError::InvalidReturnedType("AXPosition"))?;
                let size = read_size(size_value.as_ptr())
                    .ok_or(AccessibilityError::InvalidReturnedType("AXSize"))?;

                let actual = WindowBounds {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                };

                if !geometry_matches(&actual, desired) {
                    let diagnostic = format!(
                        "actual=({:.2}, {:.2}, {:.2}, {:.2}), expected=({:.2}, {:.2}, {:.2}, {:.2}), delta=({:.2}, {:.2}, {:.2}, {:.2})",
                        actual.x,
                        actual.y,
                        actual.width,
                        actual.height,
                        desired.x,
                        desired.y,
                        desired.width,
                        desired.height,
                        actual.x - desired.x,
                        actual.y - desired.y,
                        actual.width - desired.width,
                        actual.height - desired.height,
                    );

                    return Err(AccessibilityError::VerificationMismatch {
                        actual: diagnostic,
                        expected: "tolerance <= 1.00 logical point per component".to_owned(),
                    });
                }

                Ok(())
            },
            || restore_bounds_safely(pid, discovered, old_position, old_size),
        )
    }

    pub fn is_trusted() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    pub fn inspect(pid: u32) -> Result<Vec<AccessibilityWindow>, AccessibilityError> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(AccessibilityError::InvalidPid);
        }

        if !is_trusted() {
            return Err(AccessibilityError::PermissionDenied);
        }

        let application = unsafe { OwnedCf::from_raw(AXUIElementCreateApplication(pid as i32)) }
            .ok_or(AccessibilityError::ApplicationUnavailable)?;

        let windows_attribute =
            create_attribute(b"AXWindows\0").ok_or(AccessibilityError::ApplicationUnavailable)?;

        let title_attribute =
            create_attribute(b"AXTitle\0").ok_or(AccessibilityError::ApplicationUnavailable)?;

        let position_attribute =
            create_attribute(b"AXPosition\0").ok_or(AccessibilityError::ApplicationUnavailable)?;

        let size_attribute =
            create_attribute(b"AXSize\0").ok_or(AccessibilityError::ApplicationUnavailable)?;

        let windows = copy_attribute(application.as_ptr(), &windows_attribute, "AXWindows")?;

        unsafe {
            if CFGetTypeID(windows.as_ptr()) != CFArrayGetTypeID() {
                return Err(AccessibilityError::InvalidReturnedType("AXWindows"));
            }
        }

        let count = unsafe { CFArrayGetCount(windows.as_ptr()) };

        if count < 0 {
            return Err(AccessibilityError::InvalidReturnedType("AXWindows"));
        }

        let mut results = Vec::with_capacity(count as usize);

        for index in 0..count {
            let element =
                unsafe { CFArrayGetValueAtIndex(windows.as_ptr(), index) } as AXUIElementRef;

            if element.is_null() {
                continue;
            }

            let title = copy_attribute(element, &title_attribute, "AXTitle")
                .ok()
                .and_then(|value| cf_string_to_string(value.as_ptr()));

            let position = copy_attribute(element, &position_attribute, "AXPosition")
                .ok()
                .and_then(|value| read_point(value.as_ptr()));

            let size = copy_attribute(element, &size_attribute, "AXSize")
                .ok()
                .and_then(|value| read_size(value.as_ptr()));

            let bounds = match (position, size) {
                (Some(position), Some(size)) => Some(WindowBounds {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }),
                _ => None,
            };

            results.push(AccessibilityWindow {
                index: index as usize,
                title,
                bounds,
            });
        }

        Ok(results)
    }
}

pub fn is_accessibility_trusted() -> Result<bool, AccessibilityError> {
    #[cfg(target_os = "macos")]
    {
        Ok(macos::is_trusted())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(AccessibilityError::UnsupportedPlatform)
    }
}

pub fn set_application_window_bounds(
    pid: u32,
    discovered: &DiscoveredWindow,
    desired: &WindowBounds,
) -> Result<(), AccessibilityError> {
    #[cfg(target_os = "macos")]
    {
        macos::set_window_bounds(pid, discovered, desired)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, discovered, desired);
        Err(AccessibilityError::UnsupportedPlatform)
    }
}

pub fn inspect_application_windows(
    pid: u32,
) -> Result<Vec<AccessibilityWindow>, AccessibilityError> {
    #[cfg(target_os = "macos")]
    {
        macos::inspect(pid)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(AccessibilityError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod matching_tests {
    use super::*;
    use crate::window_discovery::WindowBounds;

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> WindowBounds {
        WindowBounds {
            x,
            y,
            width,
            height,
        }
    }

    fn discovered() -> DiscoveredWindow {
        DiscoveredWindow {
            window_id: 123,
            pid: 456,
            owner_name: Some("Minecraft".to_owned()),
            title: Some("Minecraft 1.21.4".to_owned()),
            bounds: bounds(100.0, 80.0, 1280.0, 720.0),
        }
    }

    fn candidate(index: usize, title: &str, geometry: WindowBounds) -> AccessibilityWindow {
        AccessibilityWindow {
            index,
            title: Some(title.to_owned()),
            bounds: Some(geometry),
        }
    }

    #[test]
    fn transaction_success_runs_all_steps_in_order() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_window_transaction(
            || {
                events.borrow_mut().push("position");
                Ok(())
            },
            || {
                events.borrow_mut().push("size");
                Ok(())
            },
            || {
                events.borrow_mut().push("verify");
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                Ok(())
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(*events.borrow(), vec!["position", "size", "verify"]);
    }

    #[test]
    fn transaction_position_failure_triggers_rollback() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let expected_error = AccessibilityError::AxCall {
            operation: "test position",
            code: -1,
        };

        let result = run_window_transaction(
            || {
                events.borrow_mut().push("position");
                Err(expected_error.clone())
            },
            || {
                events.borrow_mut().push("size");
                Ok(())
            },
            || {
                events.borrow_mut().push("verify");
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                Ok(())
            },
        );

        assert_eq!(result, Err(expected_error));
        assert_eq!(*events.borrow(), vec!["position", "rollback"]);
    }

    #[test]
    fn transaction_size_failure_triggers_rollback() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let expected_error = AccessibilityError::AxCall {
            operation: "test size",
            code: -2,
        };

        let result = run_window_transaction(
            || {
                events.borrow_mut().push("position");
                Ok(())
            },
            || {
                events.borrow_mut().push("size");
                Err(expected_error.clone())
            },
            || {
                events.borrow_mut().push("verify");
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                Ok(())
            },
        );

        assert_eq!(result, Err(expected_error));
        assert_eq!(*events.borrow(), vec!["position", "size", "rollback"]);
    }

    #[test]
    fn transaction_verification_failure_triggers_rollback() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let expected_error = AccessibilityError::VerificationFailed;

        let result = run_window_transaction(
            || {
                events.borrow_mut().push("position");
                Ok(())
            },
            || {
                events.borrow_mut().push("size");
                Ok(())
            },
            || {
                events.borrow_mut().push("verify");
                Err(expected_error.clone())
            },
            || {
                events.borrow_mut().push("rollback");
                Ok(())
            },
        );

        assert_eq!(result, Err(expected_error));
        assert_eq!(
            *events.borrow(),
            vec!["position", "size", "verify", "rollback"]
        );
    }

    #[test]
    fn transaction_rollback_failure_preserves_operation_error() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_window_transaction(
            || {
                events.borrow_mut().push("position");
                Ok(())
            },
            || {
                events.borrow_mut().push("size");
                Err(AccessibilityError::VerificationFailed)
            },
            || {
                events.borrow_mut().push("verify");
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                Err("test rollback failure")
            },
        );

        assert_eq!(
            result,
            Err(AccessibilityError::RollbackFailed {
                operation: Box::new(AccessibilityError::VerificationFailed),
                rollback: "test rollback failure",
            })
        );
        assert_eq!(*events.borrow(), vec!["position", "size", "rollback"]);
    }

    #[test]
    fn fresh_identity_accepts_unchanged_window() {
        let target = discovered();
        assert_eq!(
            verify_fresh_window_identity(&target, &[target.clone()]),
            Ok(())
        );
    }

    #[test]
    fn fresh_identity_rejects_missing_window() {
        assert_eq!(
            verify_fresh_window_identity(&discovered(), &[]),
            Err(AccessibilityError::WindowNotFound)
        );
    }

    #[test]
    fn fresh_identity_rejects_duplicate_window_id() {
        let target = discovered();
        assert_eq!(
            verify_fresh_window_identity(&target, &[target.clone(), target.clone()]),
            Err(AccessibilityError::AmbiguousWindowMatch(2))
        );
    }

    #[test]
    fn fresh_identity_rejects_pid_change() {
        let target = discovered();
        let mut actual = target.clone();
        actual.pid += 1;

        assert_eq!(
            verify_fresh_window_identity(&target, &[actual]),
            Err(AccessibilityError::ProcessMismatch {
                expected: target.pid,
                actual: target.pid + 1,
            })
        );
    }

    #[test]
    fn fresh_identity_rejects_owner_change() {
        let target = discovered();
        let mut actual = target.clone();
        actual.owner_name = Some("Different Application".to_owned());

        assert_eq!(
            verify_fresh_window_identity(&target, &[actual]),
            Err(AccessibilityError::WindowIdentityChanged("owner"))
        );
    }

    #[test]
    fn fresh_identity_rejects_title_change() {
        let target = discovered();
        let mut actual = target.clone();
        actual.title = Some("Different Window".to_owned());

        assert_eq!(
            verify_fresh_window_identity(&target, &[actual]),
            Err(AccessibilityError::WindowIdentityChanged("title"))
        );
    }

    #[test]
    fn fresh_identity_rejects_geometry_change() {
        let target = discovered();
        let mut actual = target.clone();
        actual.bounds.x += 3.0;

        assert_eq!(
            verify_fresh_window_identity(&target, &[actual]),
            Err(AccessibilityError::WindowIdentityChanged("geometry"))
        );
    }

    #[test]
    fn fresh_identity_accepts_geometry_within_two_points() {
        let target = discovered();
        let mut actual = target.clone();
        actual.bounds.x += 2.0;
        actual.bounds.width -= 1.5;

        assert_eq!(verify_fresh_window_identity(&target, &[actual]), Ok(()));
    }

    #[test]
    fn identity_only_accepts_intermediate_geometry() {
        let target = discovered();
        let mut actual = target.clone();
        actual.bounds.x += 40.0;
        actual.bounds.width -= 80.0;

        assert_eq!(
            verify_fresh_window_identity_only(&target, &[actual]),
            Ok(())
        );
    }

    #[test]
    fn identity_only_rejects_pid_change() {
        let target = discovered();
        let mut actual = target.clone();
        actual.pid += 1;

        assert_eq!(
            verify_fresh_window_identity_only(&target, &[actual]),
            Err(AccessibilityError::ProcessMismatch {
                expected: target.pid,
                actual: target.pid + 1,
            })
        );
    }

    #[test]
    fn rollback_geometry_accepts_exact_match() {
        let expected = bounds(100.0, 80.0, 1280.0, 720.0);
        assert!(geometry_matches(&expected, &expected));
    }

    #[test]
    fn rollback_geometry_accepts_one_point_difference() {
        let expected = bounds(100.0, 80.0, 1280.0, 720.0);
        let actual = bounds(101.0, 79.0, 1281.0, 719.0);
        assert!(geometry_matches(&actual, &expected));
    }

    #[test]
    fn rollback_geometry_rejects_difference_over_one_point() {
        let expected = bounds(100.0, 80.0, 1280.0, 720.0);
        let actual = bounds(101.01, 80.0, 1280.0, 720.0);
        assert!(!geometry_matches(&actual, &expected));
    }

    #[test]
    fn rollback_geometry_rejects_invalid_actual_geometry() {
        let expected = bounds(100.0, 80.0, 1280.0, 720.0);
        let actual = bounds(f64::NAN, 80.0, 1280.0, 720.0);
        assert!(!geometry_matches(&actual, &expected));
    }

    #[test]
    fn rollback_geometry_rejects_invalid_expected_geometry() {
        let actual = bounds(100.0, 80.0, 1280.0, 720.0);
        let expected = bounds(100.0, 80.0, 0.0, 720.0);
        assert!(!geometry_matches(&actual, &expected));
    }

    #[test]
    fn rollback_match_accepts_unique_title_with_intermediate_geometry() {
        let candidates = vec![candidate(
            4,
            "Minecraft 1.21.4",
            bounds(0.0, 464.0, 735.0, 492.0),
        )];

        assert_eq!(
            match_accessibility_window_for_rollback(Some("Minecraft 1.21.4"), &candidates),
            Ok(4)
        );
    }

    #[test]
    fn rollback_match_rejects_duplicate_titles() {
        let candidates = vec![
            candidate(0, "Minecraft 1.21.4", bounds(0.0, 0.0, 800.0, 600.0)),
            candidate(1, "Minecraft 1.21.4", bounds(10.0, 10.0, 800.0, 600.0)),
        ];

        assert_eq!(
            match_accessibility_window_for_rollback(Some("Minecraft 1.21.4"), &candidates),
            Err(AccessibilityError::AmbiguousWindowMatch(2))
        );
    }

    #[test]
    fn rollback_match_rejects_missing_or_invalid_geometry() {
        let missing_geometry = vec![AccessibilityWindow {
            index: 0,
            title: Some("Minecraft 1.21.4".to_owned()),
            bounds: None,
        }];

        assert_eq!(
            match_accessibility_window_for_rollback(Some("Minecraft 1.21.4"), &missing_geometry),
            Err(AccessibilityError::InvalidWindowBounds)
        );

        let invalid_geometry = vec![candidate(
            1,
            "Minecraft 1.21.4",
            bounds(0.0, 0.0, f64::NAN, 600.0),
        )];

        assert_eq!(
            match_accessibility_window_for_rollback(Some("Minecraft 1.21.4"), &invalid_geometry),
            Err(AccessibilityError::InvalidWindowBounds)
        );
    }

    #[test]
    fn rollback_match_rejects_missing_title() {
        let candidates = vec![candidate(
            0,
            "Minecraft 1.21.4",
            bounds(0.0, 0.0, 800.0, 600.0),
        )];

        assert_eq!(
            match_accessibility_window_for_rollback(None, &candidates),
            Err(AccessibilityError::WindowIdentityUnavailable)
        );
    }

    #[test]
    fn matches_exact_title_and_geometry() {
        let candidates = vec![candidate(
            3,
            "Minecraft 1.21.4",
            bounds(100.0, 80.0, 1280.0, 720.0),
        )];

        assert_eq!(
            match_accessibility_window(&discovered(), &candidates),
            Ok(3)
        );
    }

    #[test]
    fn accepts_small_geometry_difference() {
        let candidates = vec![candidate(
            2,
            "Minecraft 1.21.4",
            bounds(101.5, 79.0, 1281.0, 718.5),
        )];

        assert_eq!(
            match_accessibility_window(&discovered(), &candidates),
            Ok(2)
        );
    }

    #[test]
    fn rejects_different_title() {
        let candidates = vec![candidate(
            0,
            "Minecraft",
            bounds(100.0, 80.0, 1280.0, 720.0),
        )];

        assert_eq!(
            match_accessibility_window(&discovered(), &candidates),
            Err(AccessibilityError::WindowNotFound)
        );
    }

    #[test]
    fn rejects_geometry_outside_tolerance() {
        let candidates = vec![candidate(
            0,
            "Minecraft 1.21.4",
            bounds(103.0, 80.0, 1280.0, 720.0),
        )];

        assert_eq!(
            match_accessibility_window(&discovered(), &candidates),
            Err(AccessibilityError::WindowNotFound)
        );
    }

    #[test]
    fn rejects_ambiguous_matches() {
        let candidates = vec![
            candidate(0, "Minecraft 1.21.4", bounds(100.0, 80.0, 1280.0, 720.0)),
            candidate(1, "Minecraft 1.21.4", bounds(101.0, 80.0, 1280.0, 720.0)),
        ];

        assert_eq!(
            match_accessibility_window(&discovered(), &candidates),
            Err(AccessibilityError::AmbiguousWindowMatch(2))
        );
    }

    #[test]
    fn rejects_missing_title() {
        let mut target = discovered();
        target.title = None;

        assert_eq!(
            match_accessibility_window(&target, &[]),
            Err(AccessibilityError::WindowIdentityUnavailable)
        );
    }

    #[test]
    fn rejects_invalid_target_geometry() {
        let mut target = discovered();
        target.bounds.width = f64::NAN;

        assert_eq!(
            match_accessibility_window(&target, &[]),
            Err(AccessibilityError::WindowIdentityUnavailable)
        );
    }

    #[test]
    fn ignores_candidates_without_bounds() {
        let candidates = vec![
            AccessibilityWindow {
                index: 0,
                title: Some("Minecraft 1.21.4".to_owned()),
                bounds: None,
            },
            candidate(1, "Minecraft 1.21.4", bounds(100.0, 80.0, 1280.0, 720.0)),
        ];

        assert_eq!(
            match_accessibility_window(&discovered(), &candidates),
            Ok(1)
        );
    }
}
