//! The few Windows calls the agent needs: idle time, power source, sleep
//! inhibition and the message pump the tray icon depends on.

use std::time::Duration;
use windows_sys::Win32::System::Power::{
    ES_CONTINUOUS, ES_SYSTEM_REQUIRED, GetSystemPowerStatus, SYSTEM_POWER_STATUS,
    SetThreadExecutionState,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

/// Time since the last keyboard or mouse input in this session.
/// `None` when Windows cannot say; the caller treats that as "in use".
pub fn idle_for() -> Option<Duration> {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a valid LASTINPUTINFO with cbSize set, as the API requires.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // Both are 32-bit tick counts that wrap every 49.7 days; the wrapping
    // difference is still right across a wrap.
    // SAFETY: no arguments.
    let now = unsafe { GetTickCount() };
    Some(Duration::from_millis(u64::from(
        now.wrapping_sub(info.dwTime),
    )))
}

/// `Some(true)` on battery, `Some(false)` on AC, `None` when unknown.
pub fn on_battery() -> Option<bool> {
    // SAFETY: SYSTEM_POWER_STATUS is plain data; all-zero is a valid value.
    let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return None;
    }
    match status.ACLineStatus {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

/// While a job runs, keep Windows from sleeping; otherwise let it sleep as usual.
pub fn keep_awake(on: bool) {
    let flags = if on {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // SAFETY: plain flags; the call only changes this thread's power request.
    unsafe {
        SetThreadExecutionState(flags);
    }
}

/// Delivers pending window messages; the tray icon's hidden window needs them.
pub fn pump_messages() {
    // SAFETY: MSG is plain data; all-zero is a valid value to be filled in.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: `msg` is valid and writable; a null window means "any window of this thread".
    while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        // SAFETY: `msg` was just filled in by PeekMessageW.
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
