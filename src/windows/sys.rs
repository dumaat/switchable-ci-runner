//! The few Windows calls the agent needs: idle time, power source, sleep
//! inhibition, the hardware Auto counts on and the message pump the tray icon
//! depends on.

use std::time::Duration;
use switchable_ci_runner::parallel::Hardware;
use windows_sys::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, GetPwrCapabilities,
    GetSystemPowerStatus, SYSTEM_POWER_CAPABILITIES, SYSTEM_POWER_STATUS, SetThreadExecutionState,
};
use windows_sys::Win32::System::SystemInformation::{
    GetLogicalProcessorInformation, GetTickCount, GlobalMemoryStatusEx, MEMORYSTATUSEX,
    RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
};
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

/// True on a PC with Modern Standby (S0 low power idle), most laptops since 2020.
/// There, Windows goes to standby when the screen turns off, the lock screen's
/// short timeout included, whatever the sleep timeout says; a system-required
/// request alone did not stop it, and standby cuts the network, so the runners
/// lose GitHub in the middle of a job.
pub fn modern_standby() -> bool {
    // SAFETY: SYSTEM_POWER_CAPABILITIES is plain data; all-zero is a valid value.
    let mut caps: SYSTEM_POWER_CAPABILITIES = unsafe { std::mem::zeroed() };
    // SAFETY: `caps` is a valid, writable SYSTEM_POWER_CAPABILITIES.
    let read = unsafe { GetPwrCapabilities(&mut caps) };
    read && caps.AoAc
}

/// While a job runs, keep Windows from sleeping; otherwise let it sleep as usual.
/// With Modern Standby (`modern_standby`) that also means keeping the screen on,
/// so Windows does not enter standby; the session may still lock.
pub fn keep_awake(on: bool, modern_standby: bool) {
    let flags = match (on, modern_standby) {
        (false, _) => ES_CONTINUOUS,
        (true, false) => ES_CONTINUOUS | ES_SYSTEM_REQUIRED,
        (true, true) => ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED,
    };
    // SAFETY: plain flags; the call only changes this thread's power request.
    unsafe {
        SetThreadExecutionState(flags);
    }
}

/// Physical cores and memory, for Auto. A part Windows cannot read comes back
/// as 0, which Auto turns into one runner.
pub fn hardware() -> Hardware {
    Hardware {
        cores: physical_cores().unwrap_or(0),
        ram_bytes: total_ram().unwrap_or(0),
    }
}

fn total_ram() -> Option<u64> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..MEMORYSTATUSEX::default()
    };
    // SAFETY: `status` is a valid MEMORYSTATUSEX with dwLength set, as the API requires.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys)
}

fn physical_cores() -> Option<usize> {
    let item = size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
    let mut len: u32 = 0;
    // SAFETY: a null buffer with length 0 asks only for the size needed, written to `len`.
    unsafe { GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len) };
    let count = usize::try_from(len).ok()? / item;
    if count == 0 {
        return None;
    }
    let mut buf = vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); count];
    // SAFETY: `buf` holds `count` items, `len` bytes, as the first call asked for.
    if unsafe { GetLogicalProcessorInformation(buf.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    let filled = usize::try_from(len).ok()? / item;
    let cores = buf
        .iter()
        .take(filled)
        .filter(|i| i.Relationship == RelationProcessorCore)
        .count();
    (cores > 0).then_some(cores)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads this machine: physical cores at most the logical threads, and some memory.
    #[test]
    fn hardware_reads_this_machine() {
        let hw = hardware();
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        eprintln!(
            "{} physical cores, {threads} threads, {} GB, modern standby: {}",
            hw.cores,
            hw.ram_bytes >> 30,
            modern_standby()
        );
        assert!((1..=threads).contains(&hw.cores));
        assert!(hw.ram_bytes > 0);
    }
}
