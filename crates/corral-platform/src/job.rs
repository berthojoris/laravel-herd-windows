use std::os::windows::io::AsRawHandle;

use corral_core::error::AppError;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// Kernel Windows Job Object wrapper ensuring all child processes (`nginx.exe`, `php-cgi.exe`,
/// `mariadbd.exe`, `redis-server.exe`, `php artisan`) terminate when Corral exits or crashes (§5.5).
pub struct JobHandle {
    handle: HANDLE,
}

// SAFETY: A Windows Job Object HANDLE is thread-safe for `AssignProcessToJobObject` and `TerminateJobObject`.
unsafe impl Send for JobHandle {}
unsafe impl Sync for JobHandle {}

impl JobHandle {
    pub fn kill_on_close() -> Result<Self, AppError> {
        // SAFETY: Calling Win32 `CreateJobObjectW` with null security attributes and unnamed job.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(AppError::Io(
                "CreateJobObjectW gagal membuat Windows Job Object".to_string(),
            ));
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let info_size = u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .unwrap_or(0);

        // SAFETY: Passing valid pointer and size for `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                info_size,
            )
        };

        if ok == 0 {
            unsafe {
                CloseHandle(handle);
            }
            return Err(AppError::Io(
                "SetInformationJobObject(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE) gagal".to_string(),
            ));
        }

        Ok(Self { handle })
    }

    pub fn assign_child(&self, child: &std::process::Child) -> Result<(), AppError> {
        let raw_proc = child.as_raw_handle() as HANDLE;
        if raw_proc.is_null() {
            return Err(AppError::Io("handle proses anak tidak valid".to_string()));
        }
        // SAFETY: Both `self.handle` and `raw_proc` are valid Win32 handles.
        let ok = unsafe { AssignProcessToJobObject(self.handle, raw_proc) };
        if ok == 0 {
            tracing::warn!("AssignProcessToJobObject gagal (mungkin proses sudah berada di dalam job)");
        }
        Ok(())
    }

    pub fn terminate_all(&self) {
        if !self.handle.is_null() {
            // SAFETY: Valid Job Object handle.
            unsafe {
                TerminateJobObject(self.handle, 0);
            }
        }
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: Closing owned Job Object handle, which also triggers KILL_ON_JOB_CLOSE.
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}
