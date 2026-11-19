use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use crate::window_manager::AuthenticatedRegistration;
use crate::window_peer::PeerIdentity;

#[derive(Debug, Clone, PartialEq)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredWindow {
    pub window_id: u32,
    pub pid: u32,
    pub owner_name: Option<String>,
    pub title: Option<String>,
    pub bounds: WindowBounds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowRecord {
    pub window: DiscoveredWindow,
    pub identity: Option<PeerIdentity>,
    pub loader: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    #[cfg(not(target_os = "macos"))]
    UnsupportedPlatform,
    WindowServerUnavailable,
    InvalidWindowCount(isize),
    TooManyWindows(isize),
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(not(target_os = "macos"))]
            Self::UnsupportedPlatform => {
                write!(f, "window discovery is only supported on macOS")
            }
            Self::WindowServerUnavailable => {
                write!(f, "CoreGraphics window server is unavailable")
            }
            Self::InvalidWindowCount(count) => {
                write!(f, "CoreGraphics returned invalid window count {count}")
            }
            Self::TooManyWindows(count) => {
                write!(f, "CoreGraphics returned {count} windows; maximum is 4096")
            }
        }
    }
}

impl Error for DiscoveryError {}

/// Enumerate on-screen windows.
///
/// The returned order follows CoreGraphics' front-to-back ordering.
/// This function does not move, resize, or otherwise modify windows.
#[cfg(target_os = "macos")]
pub fn discover_windows() -> Result<Vec<DiscoveredWindow>, DiscoveryError> {
    macos::discover_windows()
}

#[cfg(not(target_os = "macos"))]
pub fn discover_windows() -> Result<Vec<DiscoveredWindow>, DiscoveryError> {
    Err(DiscoveryError::UnsupportedPlatform)
}

/// Associate discovered windows with Window Manager registrations by PID.
///
/// A process may own multiple windows. Every matching window is retained.
/// Windows without a matching registration remain in the result with
/// `loader` and `role` set to `None`.
pub fn associate_windows(
    windows: Vec<DiscoveredWindow>,
    registrations: &[AuthenticatedRegistration],
) -> Vec<WindowRecord> {
    let mut clients: HashMap<u32, Vec<&AuthenticatedRegistration>> = HashMap::new();

    for registration in registrations {
        if registration.registration.pid != registration.identity.pid
            || registration.identity.pid_version <= 0
        {
            continue;
        }

        clients
            .entry(registration.identity.pid)
            .or_default()
            .push(registration);
    }

    windows
        .into_iter()
        .map(|window| {
            let client = clients
                .get(&window.pid)
                .filter(|matches| matches.len() == 1)
                .and_then(|matches| matches.first())
                .copied();

            WindowRecord {
                window,
                identity: client.map(|entry| entry.identity.clone()),
                loader: client.map(|entry| entry.registration.loader.clone()),
                role: client.map(|entry| entry.registration.role.clone()),
            }
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{DiscoveredWindow, DiscoveryError, WindowBounds};
    use std::ffi::{CStr, c_char, c_void};

    type CFIndex = isize;
    type CFArrayRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFNumberRef = *const c_void;

    const MAX_DISCOVERED_WINDOWS: isize = 4096;
    const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
    const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

    #[repr(C)]
    #[derive(Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;

        fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;

        fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;

        fn CFNumberGetValue(number: CFNumberRef, number_type: i32, value: *mut c_void) -> u8;

        fn CFStringGetCString(
            string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> u8;

        fn CFRelease(value: *const c_void);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        static kCGWindowNumber: CFStringRef;
        static kCGWindowOwnerPID: CFStringRef;
        static kCGWindowOwnerName: CFStringRef;
        static kCGWindowName: CFStringRef;
        static kCGWindowBounds: CFStringRef;

        fn CGWindowListCopyWindowInfo(options: u32, relative_to_window: u32) -> CFArrayRef;

        fn CGRectMakeWithDictionaryRepresentation(
            dictionary: CFDictionaryRef,
            rect: *mut CGRect,
        ) -> bool;
    }

    struct OwnedCfArray(CFArrayRef);

    impl Drop for OwnedCfArray {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CFRelease(self.0);
                }
            }
        }
    }

    fn checked_window_count(count: CFIndex) -> Result<usize, DiscoveryError> {
        if count < 0 {
            return Err(DiscoveryError::InvalidWindowCount(count));
        }

        if count > MAX_DISCOVERED_WINDOWS {
            return Err(DiscoveryError::TooManyWindows(count));
        }

        usize::try_from(count).map_err(|_| DiscoveryError::InvalidWindowCount(count))
    }

    unsafe fn dictionary_value(dictionary: CFDictionaryRef, key: CFStringRef) -> *const c_void {
        // CoreGraphics supplies valid CFDictionary and CFString objects.
        unsafe { CFDictionaryGetValue(dictionary, key) }
    }

    unsafe fn read_i32(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<i32> {
        let value = unsafe { dictionary_value(dictionary, key) };

        if value.is_null() {
            return None;
        }

        let mut result = 0_i32;

        let success = unsafe {
            CFNumberGetValue(
                value,
                K_CF_NUMBER_SINT32_TYPE,
                &mut result as *mut i32 as *mut c_void,
            )
        };

        (success != 0).then_some(result)
    }

    unsafe fn read_string(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<String> {
        let value = unsafe { dictionary_value(dictionary, key) };

        if value.is_null() {
            return None;
        }

        // Window names and owner names are diagnostic metadata.
        // A bounded buffer avoids unbounded allocation from external data.
        let mut buffer = vec![0_u8; 4096];

        let success = unsafe {
            CFStringGetCString(
                value,
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
            )
        };

        if success == 0 {
            return None;
        }

        let text = unsafe { CStr::from_ptr(buffer.as_ptr() as *const c_char) };

        Some(text.to_string_lossy().into_owned())
    }

    unsafe fn read_bounds(dictionary: CFDictionaryRef) -> Option<WindowBounds> {
        let value = unsafe { dictionary_value(dictionary, kCGWindowBounds) };

        if value.is_null() {
            return None;
        }

        let mut rect = CGRect::default();

        let success =
            unsafe { CGRectMakeWithDictionaryRepresentation(value as CFDictionaryRef, &mut rect) };

        if !success {
            return None;
        }

        Some(WindowBounds {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        })
    }

    pub(super) fn discover_windows() -> Result<Vec<DiscoveredWindow>, DiscoveryError> {
        let options =
            K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS;

        let array = unsafe { CGWindowListCopyWindowInfo(options, 0) };

        if array.is_null() {
            return Err(DiscoveryError::WindowServerUnavailable);
        }

        let array = OwnedCfArray(array);
        let count = unsafe { CFArrayGetCount(array.0) };
        let capacity = checked_window_count(count)?;
        let mut windows = Vec::with_capacity(capacity);

        for index in 0..count {
            let item = unsafe { CFArrayGetValueAtIndex(array.0, index) };

            if item.is_null() {
                continue;
            }

            let dictionary = item as CFDictionaryRef;

            let window_id = match unsafe { read_i32(dictionary, kCGWindowNumber) } {
                Some(value) if value > 0 => value as u32,
                _ => continue,
            };

            let pid = match unsafe { read_i32(dictionary, kCGWindowOwnerPID) } {
                Some(value) if value > 0 => value as u32,
                _ => continue,
            };

            let bounds = match unsafe { read_bounds(dictionary) } {
                Some(value) => value,
                None => continue,
            };

            let owner_name = unsafe { read_string(dictionary, kCGWindowOwnerName) };
            let title = unsafe { read_string(dictionary, kCGWindowName) };

            windows.push(DiscoveredWindow {
                window_id,
                pid,
                owner_name,
                title,
                bounds,
            });
        }

        Ok(windows)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn window_count_is_checked_before_allocation() {
            assert_eq!(
                checked_window_count(-1),
                Err(DiscoveryError::InvalidWindowCount(-1))
            );
            assert_eq!(
                checked_window_count(MAX_DISCOVERED_WINDOWS),
                Ok(MAX_DISCOVERED_WINDOWS as usize)
            );
            assert_eq!(
                checked_window_count(MAX_DISCOVERED_WINDOWS + 1),
                Err(DiscoveryError::TooManyWindows(MAX_DISCOVERED_WINDOWS + 1))
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_manager::ClientRegistration;

    fn sample_window(window_id: u32, pid: u32) -> DiscoveredWindow {
        DiscoveredWindow {
            window_id,
            pid,
            owner_name: Some("Minecraft".to_owned()),
            title: Some("Minecraft 1.21".to_owned()),
            bounds: WindowBounds {
                x: 10.0,
                y: 20.0,
                width: 1280.0,
                height: 720.0,
            },
        }
    }

    fn registration(pid: u32, pid_version: i32, loader: &str) -> AuthenticatedRegistration {
        AuthenticatedRegistration {
            registration: ClientRegistration {
                pid,
                loader: loader.to_owned(),
                role: "minecraft_client".to_owned(),
                protocol: 1,
            },
            identity: crate::window_peer::PeerIdentity {
                pid,
                pid_version,
                audit_token: [pid, pid_version as u32, 0, 0, 0, 0, 0, 0],
            },
        }
    }

    #[test]
    fn associates_window_by_pid() {
        let windows = vec![sample_window(100, 4242)];
        let registrations = vec![registration(4242, 100, "fabric")];

        let result = associate_windows(windows, &registrations);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].loader.as_deref(), Some("fabric"));
        assert_eq!(result[0].role.as_deref(), Some("minecraft_client"));
        assert_eq!(result[0].identity, Some(registrations[0].identity.clone()));
        assert_eq!(result[0].window.window_id, 100);
    }

    #[test]
    fn keeps_unregistered_windows() {
        let windows = vec![sample_window(101, 9999)];
        let registrations = vec![registration(4242, 100, "fabric")];

        let result = associate_windows(windows, &registrations);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].identity, None);
        assert_eq!(result[0].loader, None);
        assert_eq!(result[0].role, None);
        assert_eq!(result[0].window.pid, 9999);
    }

    #[test]
    fn supports_multiple_windows_for_one_process() {
        let windows = vec![sample_window(201, 4242), sample_window(202, 4242)];
        let registrations = vec![registration(4242, 100, "neoforge")];

        let result = associate_windows(windows, &registrations);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].window.window_id, 201);
        assert_eq!(result[1].window.window_id, 202);
        assert!(
            result
                .iter()
                .all(|item| { item.loader.as_deref() == Some("neoforge") })
        );
    }

    #[test]
    fn rejects_ambiguous_pid_with_multiple_process_instances() {
        let windows = vec![sample_window(401, 4242), sample_window(402, 4242)];

        let registrations = vec![
            registration(4242, 100, "fabric"),
            registration(4242, 200, "neoforge"),
        ];

        let result = associate_windows(windows, &registrations);

        assert_eq!(result.len(), 2);

        assert!(result.iter().all(|item| {
            item.identity.is_none() && item.loader.is_none() && item.role.is_none()
        }));
    }

    #[test]
    fn rejects_registration_with_mismatched_identity_pid() {
        let windows = vec![sample_window(403, 4242)];

        let mut invalid = registration(4242, 100, "fabric");
        invalid.identity.pid = 9999;

        let result = associate_windows(windows, &[invalid]);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].identity, None);
        assert_eq!(result[0].loader, None);
        assert_eq!(result[0].role, None);
    }

    #[test]
    fn preserves_input_order() {
        let windows = vec![
            sample_window(300, 3000),
            sample_window(100, 1000),
            sample_window(200, 2000),
        ];

        let result = associate_windows(windows, &[]);

        let ids: Vec<u32> = result.iter().map(|item| item.window.window_id).collect();

        assert_eq!(ids, vec![300, 100, 200]);
    }
}
