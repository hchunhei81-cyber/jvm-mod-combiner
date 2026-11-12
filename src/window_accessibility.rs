use crate::window_discovery::WindowBounds;
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
    AxCall { operation: &'static str, code: i32 },
    InvalidReturnedType(&'static str),
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
        }
    }
}

impl Error for AccessibilityError {}

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
