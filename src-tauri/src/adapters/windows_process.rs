//! Win32 process calls behind the ssh forwards: a job object that ends the processes the
//! app spawns when the app ends, however it ends, and a process snapshot to find the
//! ones an older instance left behind. The decisions live in `ssh_tunnel`.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::ptr;
use std::sync::OnceLock;

use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, STATUS_INFO_LENGTH_MISMATCH,
    UNICODE_STRING,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    TerminateProcess,
};

use super::ssh_tunnel::Process;

/// An owned kernel handle, closed on drop.
struct Handle(HANDLE);

// Kernel handles may be used and closed from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Handle {
    fn open_process(access: u32, pid: u32) -> io::Result<Self> {
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// A job object whose processes are killed when its last handle closes.
pub struct Job(Handle);

impl Job {
    pub fn new() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(Handle(handle));
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Puts `child` in the job; processes it starts later join the job too.
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        let assigned = unsafe { AssignProcessToJobObject(self.0.0, child.as_raw_handle()) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Ties `child` to the app's lifetime: the app holds one job until it exits, and the
/// system closes its handle then, which kills every process in it.
pub fn bind_to_app(child: &Child) -> io::Result<()> {
    static APP_JOB: OnceLock<Result<Job, String>> = OnceLock::new();
    match APP_JOB.get_or_init(|| Job::new().map_err(|error| error.to_string())) {
        Ok(job) => job.assign(child),
        Err(error) => Err(io::Error::other(format!(
            "could not create the app's job object: {error}"
        ))),
    }
}

/// Every running process, without command lines or start times.
pub fn processes() -> io::Result<Vec<Process>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = Handle(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while more {
        let name = &entry.szExeFile;
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        processes.push(Process {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            exe: String::from_utf16_lossy(&name[..len]),
            command_line: None,
            started: None,
        });
        more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    Ok(processes)
}

/// The command line `pid` was started with.
pub fn command_line(pid: u32) -> io::Result<String> {
    let process = Handle::open_process(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
    let query = |buffer: &mut [u64], len: &mut u32| unsafe {
        NtQueryInformationProcess(
            process.0,
            ProcessCommandLineInformation,
            buffer.as_mut_ptr().cast(),
            u32::try_from(size_of_val(buffer)).unwrap_or(u32::MAX),
            len,
        )
    };
    let mut len = 0;
    let status = query(&mut [], &mut len);
    if status != STATUS_INFO_LENGTH_MISMATCH {
        return Err(io::Error::other(format!(
            "could not size the command line: NTSTATUS {status:#x}"
        )));
    }
    // u64 keeps the buffer aligned for the UNICODE_STRING at its start.
    let mut buffer = vec![0u64; (len as usize).div_ceil(size_of::<u64>())];
    let status = query(&mut buffer, &mut len);
    if status < 0 {
        return Err(io::Error::other(format!(
            "could not read the command line: NTSTATUS {status:#x}"
        )));
    }
    let text = unsafe { &*buffer.as_ptr().cast::<UNICODE_STRING>() };
    if text.Buffer.is_null() || text.Length == 0 {
        return Ok(String::new());
    }
    let units = unsafe { std::slice::from_raw_parts(text.Buffer, usize::from(text.Length) / 2) };
    Ok(String::from_utf16_lossy(units))
}

/// When `pid` started, in 100 ns units since 1601.
pub fn started(pid: u32) -> io::Result<u64> {
    let process = Handle::open_process(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    let read = unsafe { GetProcessTimes(process.0, created, exited, kernel, user) };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
}

pub fn terminate(pid: u32) -> io::Result<()> {
    let process = Handle::open_process(PROCESS_TERMINATE, pid)?;
    if unsafe { TerminateProcess(process.0, 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::adapters::ssh_tunnel::split_command_line;

    /// A process that keeps running for two minutes unless something ends it.
    fn long_running() -> Child {
        Command::new("cmd")
            .args(["/c", "ping", "-n", "120", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn closing_the_job_kills_its_processes() {
        let job = Job::new().unwrap();
        let mut child = long_running();
        job.assign(&child).unwrap();
        let closed = Instant::now();
        drop(job);
        child.wait().unwrap();
        assert!(closed.elapsed() < Duration::from_secs(60));
    }

    #[test]
    fn reads_and_ends_a_child_process() {
        let mut child = long_running();
        let pid = child.id();
        let found = processes()
            .unwrap()
            .into_iter()
            .find(|process| process.pid == pid)
            .unwrap();
        assert_eq!(found.parent, std::process::id());
        assert!(found.exe.eq_ignore_ascii_case("cmd.exe"));
        let line = command_line(pid).unwrap();
        assert_eq!(
            split_command_line(&line)[1..],
            ["/c", "ping", "-n", "120", "127.0.0.1"]
        );
        assert!(started(pid).unwrap() >= started(std::process::id()).unwrap());

        let ended = Instant::now();
        terminate(pid).unwrap();
        child.wait().unwrap();
        assert!(ended.elapsed() < Duration::from_secs(60));
    }
}
