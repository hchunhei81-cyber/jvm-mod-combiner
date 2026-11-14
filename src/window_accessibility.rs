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
pub struct RollbackError {
    pub stage: &'static str,
    pub detail: String,
}

impl RollbackError {
    fn new(stage: &'static str, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
        }
    }

    fn from_accessibility(stage: &'static str, error: AccessibilityError) -> Self {
        Self::new(stage, error.to_string())
    }
}

impl fmt::Display for RollbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stage, self.detail)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibilityError {
    /// All window mutations are disabled until identity authorization is complete.
    WindowMutationDisabled,
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
        rollback: RollbackError,
    },
}

impl fmt::Display for AccessibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowMutationDisabled => write!(
                f,
                "Window mutations are disabled: identity authorization and transaction safety are not yet approved"
            ),
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
    mut rollback: impl FnMut() -> Result<(), RollbackError>,
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

/// Execute a forward window transaction with an independent identity check
/// immediately before each Accessibility write.
#[cfg(any(target_os = "macos", test))]
fn run_guarded_window_transaction(
    mut verify_target: impl FnMut(&'static str) -> Result<(), AccessibilityError>,
    mut set_position: impl FnMut() -> Result<(), AccessibilityError>,
    mut set_size: impl FnMut() -> Result<(), AccessibilityError>,
    mut verify_geometry: impl FnMut() -> Result<(), AccessibilityError>,
    mut rollback: impl FnMut() -> Result<(), RollbackError>,
) -> Result<(), AccessibilityError> {
    // No write has been attempted yet. If the initial identity check fails,
    // return immediately without invoking rollback.
    verify_target("before AXPosition")?;

    // From this point onward, a setter may partially mutate the window,
    // even if the setter itself returns an error.
    let operation = (|| {
        set_position()?;
        verify_target("before AXSize")?;
        set_size()?;
        verify_geometry()
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

/// Execute rollback setters only after independently revalidating the target.
///
/// Setter failures do not suppress the next identity check. Identity failures
/// do suppress all subsequent writes. The caller remains responsible for
/// readback, even when this function returns an error.
#[cfg(any(target_os = "macos", test))]
fn run_guarded_rollback_setters(
    mut verify_target: impl FnMut(&'static str) -> Result<(), RollbackError>,
    mut set_size: impl FnMut() -> Result<(), AccessibilityError>,
    mut set_position: impl FnMut() -> Result<(), AccessibilityError>,
) -> Result<(), RollbackError> {
    let mut failures = Vec::new();

    if let Err(error) = verify_target("before AXSize rollback") {
        return Err(RollbackError::new(
            "guarding rollback setters",
            format!(
                "AXSize=not_attempted; AXPosition=not_attempted; identity validation failed before first write: {error}"
            ),
        ));
    }

    match set_size() {
        Ok(()) => {}
        Err(error) => failures.push(format!("AXSize setter failed: {error}")),
    }

    // Revalidate independently after the first setter. A successful check
    // before AXSize is not authorization to write AXPosition later.
    if let Err(error) = verify_target("before AXPosition rollback") {
        failures.push(format!(
            "AXPosition=not_attempted; identity validation failed: {error}"
        ));

        return Err(RollbackError::new(
            "guarding rollback setters",
            format!(
                "AXSize=attempted; AXPosition=not_attempted; {}",
                failures.join("; ")
            ),
        ));
    }

    match set_position() {
        Ok(()) => {}
        Err(error) => failures.push(format!("AXPosition setter failed: {error}")),
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(RollbackError::new(
            "guarding rollback setters",
            format!(
                "AXSize=attempted; AXPosition=attempted; {}",
                failures.join("; ")
            ),
        ))
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
        verify_target: impl FnMut(&'static str) -> Result<(), RollbackError>,
    ) -> Result<(), RollbackError> {
        let mut failures = Vec::new();

        // Each write has its own identity gate. A setter failure still allows
        // the next write to be considered, but an identity failure is a hard
        // stop. Readbacks below remain read-only and are always attempted.
        if let Err(error) = run_guarded_rollback_setters(
            verify_target,
            || set_size(element, size_attribute, old_size),
            || set_point(element, position_attribute, old_position),
        ) {
            failures.push(error.to_string());
        }

        // Always attempt both readbacks, even if a setter failed or was
        // skipped because the target identity could not be revalidated.
        let restored_position =
            match copy_attribute(element, position_attribute, "verifying restored AXPosition") {
                Ok(value) => match read_point(value.as_ptr()) {
                    Some(position) => Some(position),
                    None => {
                        failures.push(
                            "AXPosition readback returned an invalid or unexpected AXValue"
                                .to_owned(),
                        );
                        None
                    }
                },
                Err(error) => {
                    failures.push(format!("AXPosition readback failed: {error}"));
                    None
                }
            };

        let restored_size =
            match copy_attribute(element, size_attribute, "verifying restored AXSize") {
                Ok(value) => match read_size(value.as_ptr()) {
                    Some(size) => Some(size),
                    None => {
                        failures.push(
                            "AXSize readback returned an invalid or unexpected AXValue".to_owned(),
                        );
                        None
                    }
                },
                Err(error) => {
                    failures.push(format!("AXSize readback failed: {error}"));
                    None
                }
            };

        // Geometry can only be compared when both components were read.
        if let (Some(position), Some(size)) = (restored_position, restored_size) {
            let actual = WindowBounds {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
            };

            let expected = WindowBounds {
                x: old_position.x,
                y: old_position.y,
                width: old_size.width,
                height: old_size.height,
            };

            if !geometry_matches(&actual, &expected) {
                failures.push(format!(
                    "geometry mismatch: actual=({:.2},{:.2},{:.2},{:.2}), \
                     expected=({:.2},{:.2},{:.2},{:.2}), \
                     delta=({:.2},{:.2},{:.2},{:.2})",
                    actual.x,
                    actual.y,
                    actual.width,
                    actual.height,
                    expected.x,
                    expected.y,
                    expected.width,
                    expected.height,
                    actual.x - expected.x,
                    actual.y - expected.y,
                    actual.width - expected.width,
                    actual.height - expected.height,
                ));
            }
        }

        if !failures.is_empty() {
            return Err(RollbackError::new(
                "restoring and verifying AX bounds",
                failures.join("; "),
            ));
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
    ) -> Result<(), RollbackError> {
        run_ax_then_always_verify_cg(
            || -> Result<(), RollbackError> {
                if pid == 0 {
                    return Err(RollbackError::new(
                        "validating rollback PID",
                        "PID must be greater than zero",
                    ));
                }

                if pid > i32::MAX as u32 {
                    return Err(RollbackError::new(
                        "validating rollback PID",
                        format!("PID {pid} exceeds the supported macOS process ID range"),
                    ));
                }

                if pid != expected.pid {
                    return Err(RollbackError::new(
                        "validating rollback PID",
                        format!("PID mismatch: supplied={pid}, expected={}", expected.pid),
                    ));
                }

                let current_windows =
                    crate::window_discovery::discover_windows().map_err(|error| {
                        RollbackError::new(
                            "rediscovering window before rollback",
                            error.to_string(),
                        )
                    })?;

                verify_fresh_window_identity_only(expected, &current_windows).map_err(|error| {
                    RollbackError::new(
                        "revalidating window identity before rollback",
                        error.to_string(),
                    )
                })?;

                let application =
                    unsafe { OwnedCf::from_raw(AXUIElementCreateApplication(pid as i32)) }
                        .ok_or_else(|| {
                            RollbackError::new(
                                "creating Accessibility application for rollback",
                                "required value was unavailable",
                            )
                        })?;

                let windows_attribute = create_attribute(b"AXWindows\0").ok_or_else(|| {
                    RollbackError::new(
                        "creating AXWindows attribute",
                        "required value was unavailable",
                    )
                })?;
                let title_attribute = create_attribute(b"AXTitle\0").ok_or_else(|| {
                    RollbackError::new(
                        "creating AXTitle attribute",
                        "required value was unavailable",
                    )
                })?;
                let position_attribute = create_attribute(b"AXPosition\0").ok_or_else(|| {
                    RollbackError::new(
                        "creating AXPosition attribute",
                        "required value was unavailable",
                    )
                })?;
                let size_attribute = create_attribute(b"AXSize\0").ok_or_else(|| {
                    RollbackError::new(
                        "creating AXSize attribute",
                        "required value was unavailable",
                    )
                })?;

                // Revalidate stable identity once more immediately before restoration.
                let latest_windows =
                    crate::window_discovery::discover_windows().map_err(|error| {
                        RollbackError::new(
                            "rechecking window identity before rollback",
                            error.to_string(),
                        )
                    })?;

                verify_fresh_window_identity_only(expected, &latest_windows).map_err(|error| {
                    RollbackError::new(
                        "rechecking window identity before rollback",
                        error.to_string(),
                    )
                })?;

                let latest_window = latest_windows
                    .iter()
                    .find(|window| window.window_id == expected.window_id)
                    .ok_or_else(|| {
                        RollbackError::new(
                            "finding latest window before rollback",
                            "required value was unavailable",
                        )
                    })?;

                // Acquire AXWindows only after the latest CoreGraphics identity check.
                // Keep this array alive while using its borrowed AXUIElement.
                let windows = copy_attribute(
                    application.as_ptr(),
                    &windows_attribute,
                    "AXWindows before rollback",
                )
                .map_err(|error| {
                    RollbackError::new("reading AXWindows before rollback", error.to_string())
                })?;

                unsafe {
                    if CFGetTypeID(windows.as_ptr()) != CFArrayGetTypeID() {
                        return Err(RollbackError::new(
                            "validating AXWindows before rollback",
                            "validating AXWindows before rollback",
                        ));
                    }
                }

                let count = unsafe { CFArrayGetCount(windows.as_ptr()) };
                if count < 0 {
                    return Err(RollbackError::new(
                        "counting AXWindows before rollback",
                        "counting AXWindows before rollback",
                    ));
                }

                let mut candidates = Vec::with_capacity(count as usize);
                let mut rollback_candidate_diagnostics = Vec::with_capacity(count as usize);

                for index in 0..count {
                    let element = unsafe {
                        CFArrayGetValueAtIndex(windows.as_ptr(), index) as AXUIElementRef
                    };

                    if element.is_null() {
                        rollback_candidate_diagnostics.push(format!("index={index} element=null"));
                        continue;
                    }

                    let title_result =
                        copy_attribute(element, &title_attribute, "AXTitle before rollback");
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

                    let size_result =
                        copy_attribute(element, &size_attribute, "AXSize before rollback");
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
            latest_window.title.as_deref(),
            &candidates,
        )
        .map_err(|error| {
            eprintln!(
                "[RollbackDiagnostic] expected_title={:?} AX_count={} candidate_count={} match_error={}",
                latest_window.title,
                count,
                candidates.len(),
                error
            );

            for diagnostic in &rollback_candidate_diagnostics {
                eprintln!("[RollbackDiagnostic] {diagnostic}");
            }

            RollbackError::from_accessibility(
                "matching rollback Accessibility window",
                error,
            )
        })?;

                if selected_index >= count as usize {
                    return Err(RollbackError::new(
                        "validating rollback Accessibility index",
                        "validating rollback Accessibility index",
                    ));
                }

                let element = unsafe {
                    CFArrayGetValueAtIndex(windows.as_ptr(), selected_index as CFIndex)
                        as AXUIElementRef
                };

                if element.is_null() {
                    return Err(RollbackError::new(
                        "getting fresh Accessibility element for rollback",
                        "getting fresh Accessibility element for rollback",
                    ));
                }

                let current_title_value = copy_attribute(
                    element,
                    &title_attribute,
                    "rechecking AXTitle before rollback",
                )
                .map_err(|error| {
                    RollbackError::new("reading AXTitle before rollback", error.to_string())
                })?;

                let current_title =
                    cf_string_to_string(current_title_value.as_ptr()).ok_or_else(|| {
                        RollbackError::new(
                            "decoding AXTitle before rollback",
                            "required value was unavailable",
                        )
                    })?;

                if Some(current_title.as_str()) != latest_window.title.as_deref() {
                    return Err(RollbackError::new(
                        "revalidating Accessibility title before rollback",
                        "revalidating Accessibility title before rollback",
                    ));
                }

                let current_position_value = copy_attribute(
                    element,
                    &position_attribute,
                    "rechecking AXPosition before rollback",
                )
                .map_err(|error| {
                    RollbackError::new("reading AXPosition before rollback", error.to_string())
                })?;

                let current_size_value = copy_attribute(
                    element,
                    &size_attribute,
                    "rechecking AXSize before rollback",
                )
                .map_err(|error| {
                    RollbackError::new("reading AXSize before rollback", error.to_string())
                })?;

                let current_position =
                    read_point(current_position_value.as_ptr()).ok_or_else(|| {
                        RollbackError::new(
                            "decoding AXPosition before rollback",
                            "required value was unavailable",
                        )
                    })?;
                let current_size = read_size(current_size_value.as_ptr()).ok_or_else(|| {
                    RollbackError::new(
                        "decoding AXSize before rollback",
                        "required value was unavailable",
                    )
                })?;

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
                    return Err(RollbackError::new(
                        "validating Accessibility geometry before rollback",
                        "validating Accessibility geometry before rollback",
                    ));
                }

                attribute_is_settable(element, &position_attribute, "AXPosition").map_err(
                    |error| {
                        RollbackError::new(
                            "checking AXPosition settable before rollback",
                            error.to_string(),
                        )
                    },
                )?;
                attribute_is_settable(element, &size_attribute, "AXSize").map_err(|error| {
                    RollbackError::new(
                        "checking AXSize settable before rollback",
                        error.to_string(),
                    )
                })?;

                let verify_rollback_target = |stage: &'static str| -> Result<(), RollbackError> {
                    // CoreGraphics has no direct stable identifier that maps to
                    // this AXUIElement. Recheck the discovered Window ID, PID,
                    // owner and title, then independently re-read this element's
                    // title. This narrows, but cannot eliminate, the cross-API
                    // time-of-check/time-of-use race.
                    let fresh_windows = crate::window_discovery::discover_windows()
                        .map_err(|error| RollbackError::new(stage, error.to_string()))?;

                    verify_fresh_window_identity_only(expected, &fresh_windows)
                        .map_err(|error| RollbackError::from_accessibility(stage, error))?;

                    let title_value = copy_attribute(
                        element,
                        &title_attribute,
                        "revalidating AXTitle immediately before rollback write",
                    )
                    .map_err(|error| RollbackError::new(stage, error.to_string()))?;

                    let title = cf_string_to_string(title_value.as_ptr()).ok_or_else(|| {
                        RollbackError::new(
                            stage,
                            "AXTitle could not be decoded immediately before rollback write",
                        )
                    })?;

                    if Some(title.as_str()) != expected.title.as_deref() {
                        return Err(RollbackError::new(
                            stage,
                            format!(
                                "AXTitle changed immediately before rollback write: expected={:?}, actual={:?}",
                                expected.title, title
                            ),
                        ));
                    }

                    Ok(())
                };

                // Return the AX restoration result from this closure.
                // The outer function will always perform final CG verification.
                restore_bounds(
                    element,
                    &position_attribute,
                    &size_attribute,
                    old_position,
                    old_size,
                    verify_rollback_target,
                )
            },
            || -> Result<(), RollbackError> {
                // Verify both stable identity and restored CoreGraphics geometry.
                let restored_windows =
                    crate::window_discovery::discover_windows().map_err(|error| {
                        RollbackError::new("verifying window after rollback", error.to_string())
                    })?;

                verify_fresh_window_identity_only(expected, &restored_windows).map_err(
                    |error| {
                        RollbackError::new(
                            "verifying window identity after rollback",
                            error.to_string(),
                        )
                    },
                )?;

                let restored_window = restored_windows
                    .iter()
                    .find(|window| window.window_id == expected.window_id)
                    .ok_or_else(|| {
                        RollbackError::new(
                            "finding window after rollback",
                            "required value was unavailable",
                        )
                    })?;

                let original_bounds = WindowBounds {
                    x: old_position.x,
                    y: old_position.y,
                    width: old_size.width,
                    height: old_size.height,
                };

                if !bounds_match(&original_bounds, &restored_window.bounds) {
                    return Err(RollbackError::new(
                        "verifying restored CoreGraphics geometry",
                        "restored CoreGraphics geometry does not match original bounds",
                    ));
                }

                Ok(())
            },
        )
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

        let mut verify_forward_target = |stage: &'static str| -> Result<(), AccessibilityError> {
            let fresh_windows = crate::window_discovery::discover_windows()
                .map_err(|_| AccessibilityError::WindowDiscoveryFailed)?;

            verify_fresh_window_identity_only(discovered, &fresh_windows)?;

            let title_value = copy_attribute(
                element,
                &title_attribute,
                "revalidating AXTitle before forward write",
            )?;

            let title = cf_string_to_string(title_value.as_ptr())
                .ok_or(AccessibilityError::WindowIdentityUnavailable)?;

            if Some(title.as_str()) != discovered.title.as_deref() {
                return Err(AccessibilityError::WindowIdentityChanged(
                    "Accessibility title",
                ));
            }

            let _ = stage;
            Ok(())
        };

        run_guarded_window_transaction(
            &mut verify_forward_target,
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

fn ensure_window_mutation_enabled() -> Result<(), AccessibilityError> {
    // Fail closed. Do not remove this gate until the trusted identity and
    // transaction authorization design has been implemented and reviewed.
    Err(AccessibilityError::WindowMutationDisabled)
}

pub fn set_application_window_bounds(
    pid: u32,
    discovered: &DiscoveredWindow,
    desired: &WindowBounds,
) -> Result<(), AccessibilityError> {
    ensure_window_mutation_enabled()?;

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

fn run_ax_then_always_verify_cg(
    ax_restore: impl FnOnce() -> Result<(), RollbackError>,
    cg_verify: impl FnOnce() -> Result<(), RollbackError>,
) -> Result<(), RollbackError> {
    let ax_result = ax_restore();
    let cg_result = cg_verify();

    combine_rollback_results(ax_result, cg_result)
}

fn combine_rollback_results(
    ax_result: Result<(), RollbackError>,
    cg_result: Result<(), RollbackError>,
) -> Result<(), RollbackError> {
    match (ax_result, cg_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(ax_error), Ok(())) => Err(ax_error),
        (Ok(()), Err(cg_error)) => Err(cg_error),
        (Err(ax_error), Err(cg_error)) => Err(RollbackError::new(
            "verifying rollback recovery",
            format!(
                "AX restoration failed: {}; CoreGraphics final verification failed: {}",
                ax_error, cg_error
            ),
        )),
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
                Err(RollbackError::new("test rollback", "test rollback failure"))
            },
        );

        assert_eq!(
            result,
            Err(AccessibilityError::RollbackFailed {
                operation: Box::new(AccessibilityError::VerificationFailed),
                rollback: RollbackError::new("test rollback", "test rollback failure"),
            })
        );
        assert_eq!(*events.borrow(), vec!["position", "size", "rollback"]);
    }

    #[test]
    fn guarded_forward_transaction_checks_identity_before_each_write() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_guarded_window_transaction(
            |stage| {
                events.borrow_mut().push(stage);
                Ok(())
            },
            || {
                events.borrow_mut().push("set_position");
                Ok(())
            },
            || {
                events.borrow_mut().push("set_size");
                Ok(())
            },
            || {
                events.borrow_mut().push("verify_geometry");
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                Ok(())
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(
            *events.borrow(),
            vec![
                "before AXPosition",
                "set_position",
                "before AXSize",
                "set_size",
                "verify_geometry",
            ]
        );
    }

    #[test]
    fn guarded_forward_transaction_stops_all_writes_when_initial_identity_fails() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_guarded_window_transaction(
            |stage| {
                events.borrow_mut().push(stage);
                Err(AccessibilityError::WindowNotFound)
            },
            || {
                events.borrow_mut().push("set_position");
                Ok(())
            },
            || {
                events.borrow_mut().push("set_size");
                Ok(())
            },
            || {
                events.borrow_mut().push("verify_geometry");
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                Ok(())
            },
        );

        assert_eq!(result, Err(AccessibilityError::WindowNotFound));
        assert_eq!(*events.borrow(), vec!["before AXPosition"]);
    }

    #[test]
    fn guarded_forward_transaction_stops_second_write_when_identity_changes() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let mut checks = 0;

        let result = run_guarded_window_transaction(
            |stage| {
                events.borrow_mut().push(stage);
                checks += 1;

                if checks == 2 {
                    Err(AccessibilityError::WindowIdentityChanged("title"))
                } else {
                    Ok(())
                }
            },
            || {
                events.borrow_mut().push("set_position");
                Ok(())
            },
            || {
                events.borrow_mut().push("set_size");
                Ok(())
            },
            || {
                events.borrow_mut().push("verify_geometry");
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                Ok(())
            },
        );

        assert_eq!(
            result,
            Err(AccessibilityError::WindowIdentityChanged("title"))
        );
        assert_eq!(
            *events.borrow(),
            vec![
                "before AXPosition",
                "set_position",
                "before AXSize",
                "rollback",
            ]
        );
    }

    #[test]
    fn guarded_forward_transaction_rolls_back_after_first_setter_failure() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let expected_error = AccessibilityError::AxCall {
            operation: "test position",
            code: -10,
        };

        let result = run_guarded_window_transaction(
            |stage| {
                events.borrow_mut().push(stage);
                Ok(())
            },
            || {
                events.borrow_mut().push("set_position");
                Err(expected_error.clone())
            },
            || {
                events.borrow_mut().push("set_size");
                Ok(())
            },
            || {
                events.borrow_mut().push("verify_geometry");
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                Ok(())
            },
        );

        assert_eq!(result, Err(expected_error));
        assert_eq!(
            *events.borrow(),
            vec!["before AXPosition", "set_position", "rollback"]
        );
    }

    #[test]
    fn guarded_rollback_runs_both_writes_when_identity_is_stable() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_guarded_rollback_setters(
            |stage| {
                events.borrow_mut().push(stage);
                Ok(())
            },
            || {
                events.borrow_mut().push("set_size");
                Ok(())
            },
            || {
                events.borrow_mut().push("set_position");
                Ok(())
            },
        );

        assert_eq!(result, Ok(()));
        assert_eq!(
            *events.borrow(),
            vec![
                "before AXSize rollback",
                "set_size",
                "before AXPosition rollback",
                "set_position",
            ]
        );
    }

    #[test]
    fn guarded_rollback_continues_after_first_setter_failure() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let setter_error = AccessibilityError::VerificationFailed;

        let result = run_guarded_rollback_setters(
            |stage| {
                events.borrow_mut().push(stage);
                Ok(())
            },
            || {
                events.borrow_mut().push("set_size");
                Err(setter_error.clone())
            },
            || {
                events.borrow_mut().push("set_position");
                Ok(())
            },
        );

        let error = result.expect_err("first setter failure must be reported");
        assert!(error.detail.contains("AXSize=attempted"));
        assert!(error.detail.contains("AXPosition=attempted"));
        assert!(error.detail.contains("AXSize setter failed"));

        assert_eq!(
            *events.borrow(),
            vec![
                "before AXSize rollback",
                "set_size",
                "before AXPosition rollback",
                "set_position",
            ]
        );
    }

    #[test]
    fn guarded_rollback_stops_second_write_when_identity_changes() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_guarded_rollback_setters(
            |stage| {
                events.borrow_mut().push(stage);
                if stage == "before AXPosition rollback" {
                    return Err(RollbackError::new(
                        stage,
                        "simulated target identity change",
                    ));
                }
                Ok(())
            },
            || {
                events.borrow_mut().push("set_size");
                Ok(())
            },
            || {
                events.borrow_mut().push("set_position");
                Ok(())
            },
        );

        let error = result.expect_err("identity change must stop later writes");
        assert!(error.detail.contains("AXSize=attempted"));
        assert!(error.detail.contains("AXPosition=not_attempted"));
        assert!(error.detail.contains("simulated target identity change"));

        assert_eq!(
            *events.borrow(),
            vec![
                "before AXSize rollback",
                "set_size",
                "before AXPosition rollback",
            ]
        );
    }

    #[test]
    fn guarded_rollback_performs_no_writes_when_initial_identity_check_fails() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_guarded_rollback_setters(
            |stage| {
                events.borrow_mut().push(stage);
                Err(RollbackError::new(
                    stage,
                    "simulated initial identity failure",
                ))
            },
            || {
                events.borrow_mut().push("set_size");
                Ok(())
            },
            || {
                events.borrow_mut().push("set_position");
                Ok(())
            },
        );

        let error = result.expect_err("initial identity failure must block writes");
        assert!(error.detail.contains("AXSize=not_attempted"));
        assert!(error.detail.contains("AXPosition=not_attempted"));

        assert_eq!(*events.borrow(), vec!["before AXSize rollback"]);
    }

    #[test]
    fn ax_preparation_failure_still_runs_cg_verification() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_ax_then_always_verify_cg(
            || {
                events.borrow_mut().push("ax_preparation");
                Err(RollbackError::new(
                    "AX preparation",
                    "simulated AXWindows retrieval failure",
                ))
            },
            || {
                events.borrow_mut().push("cg_verification");
                Ok(())
            },
        );

        let error = result.expect_err("AX preparation failure must be preserved");
        assert_eq!(error.stage, "AX preparation");
        assert!(error.detail.contains("AXWindows retrieval failure"));
        assert_eq!(*events.borrow(), vec!["ax_preparation", "cg_verification"]);
    }

    #[test]
    fn ax_restore_failure_still_runs_cg_verification() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_ax_then_always_verify_cg(
            || {
                events.borrow_mut().push("ax_restore");
                Err(RollbackError::new(
                    "AX restore",
                    "simulated AX setter failure",
                ))
            },
            || {
                events.borrow_mut().push("cg_verification");
                Ok(())
            },
        );

        let error = result.expect_err("AX restore failure must be preserved");
        assert_eq!(error.stage, "AX restore");
        assert!(error.detail.contains("AX setter failure"));
        assert_eq!(*events.borrow(), vec!["ax_restore", "cg_verification"]);
    }

    #[test]
    fn ax_and_cg_failures_are_both_preserved_and_cg_runs() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());

        let result = run_ax_then_always_verify_cg(
            || {
                events.borrow_mut().push("ax_restore");
                Err(RollbackError::new("AX restore", "simulated AX failure"))
            },
            || {
                events.borrow_mut().push("cg_verification");
                Err(RollbackError::new(
                    "CG verification",
                    "simulated final snapshot failure",
                ))
            },
        );

        let error = result.expect_err("both failures must be reported");
        assert_eq!(error.stage, "verifying rollback recovery");
        assert!(error.detail.contains("simulated AX failure"));
        assert!(error.detail.contains("simulated final snapshot failure"));
        assert_eq!(*events.borrow(), vec!["ax_restore", "cg_verification"]);
    }

    #[test]
    fn rollback_result_combiner_accepts_two_successes() {
        assert_eq!(combine_rollback_results(Ok(()), Ok(())), Ok(()));
    }

    #[test]
    fn rollback_result_combiner_preserves_ax_failure() {
        let ax_error = RollbackError::new("AX restore", "simulated AX failure");

        let result = combine_rollback_results(Err(ax_error.clone()), Ok(()));

        assert_eq!(result, Err(ax_error));
    }

    #[test]
    fn rollback_result_combiner_preserves_cg_failure() {
        let cg_error = RollbackError::new("CG verification", "simulated CG failure");

        let result = combine_rollback_results(Ok(()), Err(cg_error.clone()));

        assert_eq!(result, Err(cg_error));
    }

    #[test]
    fn rollback_result_combiner_preserves_both_failures() {
        let ax_error = RollbackError::new("AX restore", "simulated AX failure");
        let cg_error = RollbackError::new("CG verification", "simulated CG failure");

        let result = combine_rollback_results(Err(ax_error), Err(cg_error));

        let error = result.expect_err("both failures must be reported");

        assert_eq!(error.stage, "verifying rollback recovery");
        assert!(error.detail.contains("AX restoration failed"));
        assert!(error.detail.contains("simulated AX failure"));
        assert!(
            error
                .detail
                .contains("CoreGraphics final verification failed")
        );
        assert!(error.detail.contains("simulated CG failure"));
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

#[cfg(test)]
mod mutation_gate_tests {
    use super::*;

    #[test]
    fn window_mutation_gate_fails_closed() {
        let result = ensure_window_mutation_enabled();

        assert_eq!(result, Err(AccessibilityError::WindowMutationDisabled));
    }

    #[test]
    fn window_mutation_disabled_error_is_explicit() {
        let message = AccessibilityError::WindowMutationDisabled.to_string();

        assert!(message.contains("Window mutations are disabled"));
        assert!(message.contains("identity authorization"));
    }
}
