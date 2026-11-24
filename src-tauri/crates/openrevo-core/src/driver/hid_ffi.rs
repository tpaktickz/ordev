// OpenRevo - Unified Windows Win32 HID, SetupAPI, Kernel32 & User32 FFI Layer
// Extracted to centralize low-level hardware communication and eliminate duplication.

#![allow(dead_code, non_snake_case, non_camel_case_types)]

use std::ffi::c_void;

pub type HANDLE = *mut c_void;
pub type BOOLEAN = u8;

pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
pub const DIGCF_PRESENT: u32 = 0x00000002;
pub const DIGCF_DEVICEINTERFACE: u32 = 0x00000010;
pub const GENERIC_READ: u32 = 0x80000000;
pub const GENERIC_WRITE: u32 = 0x40000000;
pub const FILE_SHARE_READ: u32 = 0x00000001;
pub const FILE_SHARE_WRITE: u32 = 0x00000002;
pub const OPEN_EXISTING: u32 = 3;
pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
pub const FILE_FLAG_OVERLAPPED: u32 = 0x40000000;
pub const ERROR_IO_PENDING: u32 = 997;
pub const WAIT_OBJECT_0: u32 = 0;

#[repr(C)]
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct HIDD_ATTRIBUTES {
    pub size: u32,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_number: u16,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct HIDP_CAPS {
    pub usage: u16,
    pub usage_page: u16,
    pub input_report_byte_length: u16,
    pub output_report_byte_length: u16,
    pub feature_report_byte_length: u16,
    pub reserved: [u16; 17],
    pub number_link_collection_nodes: u16,
    pub number_input_button_caps: u16,
    pub number_input_value_caps: u16,
    pub number_input_data_indices: u16,
    pub number_output_button_caps: u16,
    pub number_output_value_caps: u16,
    pub number_output_data_indices: u16,
    pub number_feature_button_caps: u16,
    pub number_feature_value_caps: u16,
    pub number_feature_data_indices: u16,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct SP_DEVICE_INTERFACE_DATA {
    pub cb_size: u32,
    pub interface_class_guid: GUID,
    pub flags: u32,
    pub reserved: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct LARGE_INTEGER {
    pub quad_part: i64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct OVERLAPPED {
    pub internal: usize,
    pub internal_high: usize,
    pub offset: u32,
    pub offset_high: u32,
    pub h_event: HANDLE,
}

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct LASTINPUTINFO {
    pub cb_size: u32,
    pub dw_time: u32,
}

#[cfg(windows)]
#[link(name = "hid")]
extern "system" {
    pub fn HidD_GetHidGuid(hid_guid: *mut GUID);
    pub fn HidD_GetAttributes(hid_device_object: HANDLE, attributes: *mut HIDD_ATTRIBUTES) -> BOOLEAN;
    pub fn HidD_GetPreparsedData(hid_device_object: HANDLE, preparsed_data: *mut isize) -> BOOLEAN;
    pub fn HidD_FreePreparsedData(preparsed_data: isize) -> BOOLEAN;
    pub fn HidP_GetCaps(preparsed_data: isize, capabilities: *mut HIDP_CAPS) -> i32;
    pub fn HidD_SetFeature(
        hid_device_object: HANDLE,
        report_buffer: *const c_void,
        report_buffer_length: u32,
    ) -> BOOLEAN;
    pub fn HidD_GetFeature(
        hid_device_object: HANDLE,
        report_buffer: *mut c_void,
        report_buffer_length: u32,
    ) -> BOOLEAN;
}

#[cfg(windows)]
#[link(name = "setupapi")]
extern "system" {
    pub fn SetupDiGetClassDevsW(
        class_guid: *const GUID,
        enumerator: *const u16,
        hwnd_parent: *mut c_void,
        flags: u32,
    ) -> *mut c_void;
    pub fn SetupDiEnumDeviceInterfaces(
        device_info_set: *mut c_void,
        device_info_data: *mut c_void,
        interface_class_guid: *const GUID,
        member_index: u32,
        device_interface_data: *mut SP_DEVICE_INTERFACE_DATA,
    ) -> i32;
    pub fn SetupDiGetDeviceInterfaceDetailW(
        device_info_set: *mut c_void,
        device_interface_data: *mut SP_DEVICE_INTERFACE_DATA,
        device_interface_detail_data: *mut u8,
        device_interface_detail_data_size: u32,
        required_size: *mut u32,
        device_info_data: *mut c_void,
    ) -> i32;
    pub fn SetupDiDestroyDeviceInfoList(device_info_set: *mut c_void) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    pub fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut c_void,
    ) -> HANDLE;
    pub fn CloseHandle(object: HANDLE) -> i32;
    pub fn WriteFile(
        file: HANDLE,
        buffer: *const c_void,
        number_of_bytes_to_write: u32,
        number_of_bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn CreateEventW(
        lpEventAttributes: *mut c_void,
        bManualReset: i32,
        bInitialState: i32,
        lpName: *const u16,
    ) -> HANDLE;
    pub fn WaitForSingleObject(hHandle: HANDLE, dwMilliseconds: u32) -> u32;
    pub fn GetOverlappedResult(
        hFile: HANDLE,
        lpOverlapped: *mut c_void,
        lpNumberOfBytesTransferred: *mut u32,
        bWait: i32,
    ) -> i32;
    pub fn ResetEvent(hEvent: HANDLE) -> i32;
    pub fn CancelIoEx(hFile: HANDLE, lpOverlapped: *mut c_void) -> i32;
    pub fn QueryPerformanceCounter(lpPerformanceCount: *mut LARGE_INTEGER) -> i32;
    pub fn QueryPerformanceFrequency(lpFrequency: *mut LARGE_INTEGER) -> i32;
    pub fn GetLastError() -> u32;
    pub fn GetTickCount() -> u32;
    pub fn GetCurrentThread() -> HANDLE;
    pub fn SetThreadPriority(hThread: HANDLE, nPriority: i32) -> i32;
}

pub const THREAD_PRIORITY_ABOVE_NORMAL: i32 = 1;
pub const THREAD_PRIORITY_HIGHEST: i32 = 2;

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    pub fn GetLastInputInfo(plii: *mut LASTINPUTINFO) -> i32;
    pub fn GetAsyncKeyState(v_key: i32) -> i16;
}

/// Calculate system idle duration in milliseconds using GetLastInputInfo and GetTickCount
#[cfg(windows)]
pub fn get_system_idle_ms() -> u64 {
    unsafe {
        let mut lii = LASTINPUTINFO {
            cb_size: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dw_time: 0,
        };
        if GetLastInputInfo(&mut lii) != 0 {
            let now = GetTickCount();
            if now >= lii.dw_time {
                (now - lii.dw_time) as u64
            } else {
                (u32::MAX - lii.dw_time + now) as u64
            }
        } else {
            0
        }
    }
}

/// Query whether a virtual key is currently pressed
#[inline]
#[cfg(windows)]
pub fn is_key_down(v_key: i32) -> bool {
    unsafe { (GetAsyncKeyState(v_key) as u16 & 0x8000) != 0 }
}

// ---------------------------------------------------------------------------
// Linux HID transport
//
// The Win32 HID stack (hid.dll / setupapi / kernel32 device handles) has no
// equivalent here. Linux exposes the same devices through hidraw; that backend
// is written in P2 of the port plan. Until it lands, every entry point keeps
// its exact Win32 signature and fails the way a missing device would, so the
// callers (lighting, streamer, BLD driver) compile and their error paths run.
//
// Windows error semantics reproduced here:
//   * handle-returning calls  -> INVALID_HANDLE_VALUE
//   * BOOL-returning calls    -> 0 (FALSE)
//   * GetLastError()          -> ERROR_FILE_NOT_FOUND (2)
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
pub const ERROR_FILE_NOT_FOUND: u32 = 2;

#[cfg(not(windows))]
pub mod hidraw_placeholder {
    // Marker module: keeps the split obvious when the real backend is added.
    pub const BACKEND: &str = "none";
}

#[cfg(not(windows))]
mod linux_ffi {
    use super::*;

    #[inline]
    fn no_device<T: Default>(_what: &str) -> T {
        T::default()
    }

    #[allow(clippy::too_many_arguments, unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn HidD_GetHidGuid(hid_guid: *mut GUID) {
        let _ = no_device::<u8>("HidD_GetHidGuid");
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn HidD_GetAttributes(
        hid_device_object: HANDLE,
        attributes: *mut HIDD_ATTRIBUTES,
    ) -> BOOLEAN {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn HidD_GetPreparsedData(
        hid_device_object: HANDLE,
        preparsed_data: *mut isize,
    ) -> BOOLEAN {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn HidD_FreePreparsedData(preparsed_data: isize) -> BOOLEAN {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn HidP_GetCaps(preparsed_data: isize, capabilities: *mut HIDP_CAPS) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn HidD_SetFeature(
        hid_device_object: HANDLE,
        report_buffer: *const c_void,
        report_buffer_length: u32,
    ) -> BOOLEAN {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn HidD_GetFeature(
        hid_device_object: HANDLE,
        report_buffer: *mut c_void,
        report_buffer_length: u32,
    ) -> BOOLEAN {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn SetupDiGetClassDevsW(
        class_guid: *const GUID,
        enumerator: *const u16,
        hwnd_parent: *mut c_void,
        flags: u32,
    ) -> *mut c_void {
        std::ptr::null_mut()
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn SetupDiEnumDeviceInterfaces(
        device_info_set: *mut c_void,
        device_info_data: *mut c_void,
        interface_class_guid: *const GUID,
        member_index: u32,
        device_interface_data: *mut SP_DEVICE_INTERFACE_DATA,
    ) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn SetupDiGetDeviceInterfaceDetailW(
        device_info_set: *mut c_void,
        device_interface_data: *mut SP_DEVICE_INTERFACE_DATA,
        device_interface_detail_data: *mut u8,
        device_interface_detail_data_size: u32,
        required_size: *mut u32,
        device_info_data: *mut c_void,
    ) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn SetupDiDestroyDeviceInfoList(device_info_set: *mut c_void) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut c_void,
    ) -> HANDLE {
        INVALID_HANDLE_VALUE
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn CloseHandle(object: HANDLE) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn WriteFile(
        file: HANDLE,
        buffer: *const c_void,
        number_of_bytes_to_write: u32,
        number_of_bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn CreateEventW(
        lpEventAttributes: *mut c_void,
        bManualReset: i32,
        bInitialState: i32,
        lpName: *const u16,
    ) -> HANDLE {
        INVALID_HANDLE_VALUE
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn WaitForSingleObject(hHandle: HANDLE, dwMilliseconds: u32) -> u32 {
        0x0000_0102 // WAIT_FAILED
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn GetOverlappedResult(
        hFile: HANDLE,
        lpOverlapped: *mut c_void,
        lpNumberOfBytesTransferred: *mut u32,
        bWait: i32,
    ) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn ResetEvent(hEvent: HANDLE) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn CancelIoEx(hFile: HANDLE, lpOverlapped: *mut c_void) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn QueryPerformanceCounter(lpPerformanceCount: *mut LARGE_INTEGER) -> i32 {
        1
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn QueryPerformanceFrequency(lpFrequency: *mut LARGE_INTEGER) -> i32 {
        if !lpFrequency.is_null() {
            // Nanoseconds clock exposed as counts-per-second.
            (*lpFrequency).quad_part = 1_000_000_000;
        }
        1
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn GetLastError() -> u32 {
        ERROR_FILE_NOT_FOUND
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn GetTickCount() -> u32 {
        let uptime = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
            .unwrap_or(0.0);
        (uptime * 1000.0) as u32
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn GetCurrentThread() -> HANDLE {
        std::ptr::null_mut()
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn SetThreadPriority(hThread: HANDLE, nPriority: i32) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn GetLastInputInfo(plii: *mut LASTINPUTINFO) -> i32 {
        0
    }

    #[allow(unused_variables, clippy::missing_safety_doc)]
    pub unsafe fn GetAsyncKeyState(v_key: i32) -> i16 {
        0
    }
}

#[cfg(not(windows))]
pub use linux_ffi::*;

/// Idle time is not globally observable on Linux without evdev; report 0 (busy).
#[cfg(not(windows))]
#[inline]
pub fn get_system_idle_ms() -> u64 {
    0
}

/// Linux: no global key-state poll until the evdev hook lands (P2).
#[cfg(not(windows))]
#[inline]
pub fn is_key_down(_v_key: i32) -> bool {
    false
}

// --- OEM GamingCenter compatibility synchronization ---
// These used to talk to the Win32 registry directly; the engine only needs the
// key/value shape, so they now go through the portable registry surface (no-ops
// on Linux, where nothing reads these keys).

pub fn set_hklm_dword(sub_key: &str, value_name: &str, value: u32) {
    crate::pal::reg::set_dword_or_create(crate::pal::reg::RegRoot::Machine, sub_key, value_name, value);
}

pub fn set_hklm_sz(sub_key: &str, value_name: &str, value: &str) {
    crate::pal::reg::set_string(crate::pal::reg::RegRoot::Machine, sub_key, value_name, value);
}

pub fn read_hklm_dword(sub_key: &str, value_name: &str) -> Option<u32> {
    crate::pal::reg::read_dword(crate::pal::reg::RegRoot::Machine, sub_key, value_name)
}
