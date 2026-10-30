//! A Windows job object around the runner: every process a CI job starts
//! (cargo, compilers, tests) joins it and gets the lowest priority ("Idle"),
//! plus an optional hard CPU cap. Closing the job kills what is left in it,
//! so nothing outlives the agent.

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use switchable_ci_runner::cpu::hard_cap_rate;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
    JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PRIORITY_CLASS, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::IDLE_PRIORITY_CLASS;

pub struct Job(HANDLE);

impl Job {
    pub fn new(cpu_limit_percent: u8) -> Result<Job, String> {
        // SAFETY: null attributes and name create an anonymous job with default security.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(format!("cannot create a job object: {}", last_error()));
        }
        let job = Job(handle);

        // SAFETY: plain data; all-zero means "no limits" before we set ours.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_PRIORITY_CLASS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        limits.BasicLimitInformation.PriorityClass = IDLE_PRIORITY_CLASS;
        job.set(
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const core::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
        )?;

        if let Some(rate) = hard_cap_rate(cpu_limit_percent) {
            // SAFETY: plain data, filled in below.
            let mut cap: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
            cap.ControlFlags =
                JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
            cap.Anonymous.CpuRate = rate;
            job.set(
                JobObjectCpuRateControlInformation,
                &cap as *const _ as *const core::ffi::c_void,
                size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>(),
            )?;
        }
        Ok(job)
    }

    fn set(
        &self,
        class: windows_sys::Win32::System::JobObjects::JOBOBJECTINFOCLASS,
        info: *const core::ffi::c_void,
        len: usize,
    ) -> Result<(), String> {
        // SAFETY: `info` points to a live struct of `len` bytes matching `class`.
        if unsafe { SetInformationJobObject(self.0, class, info, len as u32) } == 0 {
            return Err(format!("cannot set the job limits: {}", last_error()));
        }
        Ok(())
    }

    /// Puts the runner in the job; the processes it starts later join it too.
    pub fn assign(&self, child: &Child) -> Result<(), String> {
        // SAFETY: both handles are valid for the duration of the call.
        if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) } == 0 {
            return Err(format!(
                "cannot put the runner in the job: {}",
                last_error()
            ));
        }
        Ok(())
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use windows_sys::Win32::System::Threading::GetPriorityClass;

    /// The real job object, on this machine: a child put in it with a cap
    /// runs at the Idle priority class.
    #[test]
    fn child_in_the_job_runs_at_idle_priority() {
        let job = Job::new(25).unwrap();
        let mut child = Command::new("ping")
            .args(["-n", "3", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        job.assign(&child).unwrap();
        // SAFETY: the child handle is valid until wait() below.
        let class = unsafe { GetPriorityClass(child.as_raw_handle() as HANDLE) };
        assert_eq!(class, IDLE_PRIORITY_CLASS);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn no_cap_still_creates_the_job() {
        assert!(Job::new(100).is_ok());
    }
}
