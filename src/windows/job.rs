//! Windows job objects around the runners. One shared job holds every runner:
//! every process a CI job starts (cargo, compilers, tests) joins it and gets
//! the lowest priority ("Idle"), plus an optional hard CPU cap for all of them
//! together. Inside it, each runner has its own job (nested), so stopping one
//! runner kills what its jobs left behind and nothing of the others.
//! Closing a job kills what is left in it, so nothing outlives the agent.

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use switchable_ci_runner::cpu::hard_cap_rate;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
    JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PRIORITY_CLASS, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::IDLE_PRIORITY_CLASS;

pub struct Job(HANDLE);

impl Job {
    fn create() -> Result<Job, String> {
        // SAFETY: null attributes and name create an anonymous job with default security.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(format!("cannot create a job object: {}", last_error()));
        }
        Ok(Job(handle))
    }

    /// The job shared by all runners: Idle priority and the optional CPU cap.
    pub fn shared(cpu_limit_percent: u8) -> Result<Job, String> {
        let job = Job::create()?;
        job.limits(
            JOB_OBJECT_LIMIT_PRIORITY_CLASS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            IDLE_PRIORITY_CLASS,
        )?;
        job.set_cpu_cap(cpu_limit_percent)?;
        Ok(job)
    }

    /// One runner's own job, nested in the shared one when the runner joins both.
    pub fn per_runner() -> Result<Job, String> {
        let job = Job::create()?;
        job.limits(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, 0)?;
        Ok(job)
    }

    fn limits(&self, flags: u32, priority: u32) -> Result<(), String> {
        // SAFETY: plain data; all-zero means "no limits" before we set ours.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = flags;
        limits.BasicLimitInformation.PriorityClass = priority;
        self.set(
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const core::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
        )
    }

    /// Changes the cap at once, for the jobs already running too; 100 removes it.
    pub fn set_cpu_cap(&self, cpu_limit_percent: u8) -> Result<(), String> {
        // SAFETY: plain data, filled in below.
        let mut cap: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
        cap.ControlFlags =
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
        // Windows refuses all-zero flags ("no cap"), so no cap is a cap of 100%
        // (10 000 hundredths of a percent): the same thing, and it accepts it.
        cap.Anonymous.CpuRate = hard_cap_rate(cpu_limit_percent).unwrap_or(10_000);
        self.set(
            JobObjectCpuRateControlInformation,
            &cap as *const _ as *const core::ffi::c_void,
            size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>(),
        )
    }

    /// Processes still in the job; `None` when Windows cannot say.
    pub fn active_processes(&self) -> Option<u32> {
        // SAFETY: plain data, filled in by the call.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a live, writable struct of the size passed.
        let ok = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(info.ActiveProcesses)
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
    /// A process already in a job makes this one nested inside that job.
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

    fn ping() -> Child {
        Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    /// The real job objects, on this machine: a child in the shared job and
    /// then in its own runs at the Idle priority class, and closing its own
    /// job kills it without touching the shared one.
    #[test]
    fn nested_jobs_keep_idle_priority_and_kill_only_their_own() {
        let shared = Job::shared(25).unwrap();
        let mut child = ping();
        shared.assign(&child).unwrap();
        let own = Job::per_runner().unwrap();
        own.assign(&child).unwrap();
        // SAFETY: the child handle is valid until wait() below.
        let class = unsafe { GetPriorityClass(child.as_raw_handle() as HANDLE) };
        assert_eq!(class, IDLE_PRIORITY_CLASS);
        assert_eq!(own.active_processes(), Some(1));
        shared.set_cpu_cap(50).unwrap();
        shared.set_cpu_cap(100).unwrap();
        let t = std::time::Instant::now();
        drop(own);
        child.wait().unwrap();
        // The ping would take 29 s; killed on close it ends at once (with exit code 0,
        // which is what kill-on-close sets, so the time is what tells).
        assert!(t.elapsed() < std::time::Duration::from_secs(10));
        // The shared job's count follows the exit a moment later.
        let mut left = shared.active_processes();
        for _ in 0..50 {
            if left == Some(0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            left = shared.active_processes();
        }
        assert_eq!(left, Some(0));
    }

    #[test]
    fn no_cap_still_creates_the_job() {
        assert!(Job::shared(100).is_ok());
    }
}
