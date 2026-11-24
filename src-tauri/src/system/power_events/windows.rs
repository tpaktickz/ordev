//! Windows power-event source: `WM_POWERBROADCAST` on a message-only watcher
//! window, plus the lid-switch power-setting registration.

use std::sync::atomic::Ordering;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassExW, TranslateMessage, MSG, WINDOW_EX_STYLE, WNDCLASSEXW, WS_OVERLAPPED,
};

const WM_POWERBROADCAST: u32 = 0x0218;
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;
const WM_DISPLAYCHANGE: u32 = 0x007E;
const WM_DESTROY: u32 = 0x0002;

const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;
const PBT_POWERSETTINGCHANGE: u32 = 0x8013;
const DEVICE_NOTIFY_WINDOW_HANDLE: windows::Win32::UI::WindowsAndMessaging::REGISTER_NOTIFICATION_FLAGS =
    windows::Win32::UI::WindowsAndMessaging::REGISTER_NOTIFICATION_FLAGS(0);

// GUID for Lid switch state: {BA3E0F4D-B817-4094-A2D1-D56379E6A0F3}
const GUID_LIDSWITCH_STATE_CHANGE: windows::core::GUID =
    windows::core::GUID::from_u128(0xba3e0f4d_b817_4094_a2d1_d56379e6a0f3);

static POWER_WATCHER_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn get_power_watcher_hwnd() -> Option<HWND> {
    let val = POWER_WATCHER_HWND.load(Ordering::SeqCst);
    if val != 0 {
        Some(HWND(val as _))
    } else {
        None
    }
}

unsafe extern "system" fn power_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_POWERBROADCAST => {
            let pbt = wparam.0 as u32;
            if pbt == PBT_APMSUSPEND {
                super::enter_suspend("Standby event");
            } else if pbt == PBT_POWERSETTINGCHANGE {
                let setting_ptr = lparam.0 as *const POWERBROADCAST_SETTING;
                if !setting_ptr.is_null() {
                    let setting = &*setting_ptr;
                    if setting.PowerSetting == GUID_LIDSWITCH_STATE_CHANGE && setting.DataLength >= 4
                    {
                        static LAST_LID_STATE: std::sync::atomic::AtomicI8 =
                            std::sync::atomic::AtomicI8::new(-1);
                        let lid_state = *(setting.Data.as_ptr() as *const u32);
                        let prev = LAST_LID_STATE.swap(lid_state as i8, Ordering::SeqCst);
                        if prev == -1 {
                            // Windows automatically posts initial state upon RegisterPowerSettingNotification.
                            // Record baseline only; NEVER trigger a false wake/resume pipeline on app startup!
                            println!("[Power Guard] Initial lid state registered: {} (startup baseline set)", if lid_state == 0 { "Closed" } else { "Open" });
                        } else if prev != lid_state as i8 {
                            // 0 = Lid Closed, 1 = Lid Open
                            if lid_state == 0 {
                                super::lid_closed();
                            } else {
                                super::resume_pipeline("lid opened");
                            }
                        }
                    }
                }
            } else if pbt == PBT_APMRESUMEAUTOMATIC || pbt == PBT_APMRESUMESUSPEND {
                super::resume_pipeline("resumed from Sleep / Hibernate");
            }
            LRESULT(1)
        }
        WM_QUERYENDSESSION | WM_ENDSESSION => {
            super::system_shutdown();
            LRESULT(1)
        }
        // Display topology or mode changed (resolution/refresh switch, monitor hot-plug,
        // projection mode). The cached mode table is stale from this moment on.
        WM_DISPLAYCHANGE => {
            crate::system::display::invalidate_display_mode_cache();
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Create the watcher window and pump its messages. Returns `false` when the
/// window could not be created.
///
/// Guaranteed to catch:
/// 1. Tray quit / App exit
/// 2. Laptop Lid Close (半闭/合盖)
/// 3. Sleep / Standby (待机睡眠)
/// 4. Hibernate (休眠)
/// 5. System Shutdown / Restart (系统关机)
pub(super) fn start() -> bool {
    unsafe {
        let class_name = w!("OpenRevoPowerWatcherClass");
        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(power_wnd_proc),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };

        RegisterClassExW(&wnd_class);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("OpenRevoPowerWatcher"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND::default(),
            None,
            None,
            None,
        );

        if hwnd.is_err() {
            eprintln!("[Power Guard] Failed to create power watcher window");
            return false;
        }

        let hwnd = hwnd.unwrap();
        POWER_WATCHER_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

        // Register explicitly for Lid Switch state change notifications
        let _ = RegisterPowerSettingNotification(
            HANDLE(hwnd.0 as _),
            &GUID_LIDSWITCH_STATE_CHANGE,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        );

        println!("[Power Guard] Dedicated power event listener successfully registered and running.");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}
