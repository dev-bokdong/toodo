//! Reverse port forwards that let programs on an SSH machine reach a loopback port of
//! this machine, kept open by `ssh -N -R`.
//!
//! On Windows nothing ends a child process with its parent, so a forward would outlive
//! an app that crashed or was ended from Task Manager and keep the remote port taken.
//! Each ssh therefore joins a job object that the system closes with the app, and
//! before opening a forward the app ends leftover forwards of instances that are gone.

use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::domain::model::MachineLabel;

/// How one forward is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardState {
    /// ssh is connecting and has not confirmed the forward yet.
    Connecting,
    /// The remote port forwards to this machine.
    Open,
    /// ssh exited; carries why, e.g. the remote port is taken.
    Closed(String),
}

/// A running forward; dropping it stops the forward.
pub trait Forward: Send {
    fn state(&mut self) -> ForwardState;
}

pub trait Forwarder: Send + Sync {
    /// Makes `127.0.0.1:<port>` on `host` forward to `127.0.0.1:<port>` here.
    fn open(&self, host: &MachineLabel, port: u16) -> io::Result<Box<dyn Forward>>;
}

/// Forwards with the `ssh` client, sharing the user's `~/.ssh/config` and agent.
pub struct SshForwarder;

/// The ssh arguments of a forward. `-v` makes ssh report when the server accepted the
/// forward; `ExitOnForwardFailure` makes it exit when it did not, e.g. because the
/// remote port is taken. A dedicated connection (`ControlPath=none`) keeps the forward
/// from outliving the process in a shared master connection.
pub fn forward_args(host: &MachineLabel, port: u16) -> Vec<String> {
    let forward = format!("127.0.0.1:{port}:127.0.0.1:{port}");
    [
        "-v",
        "-N",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ControlPath=none",
        "-R",
        &forward,
        "--",
        host.as_str(),
    ]
    .map(str::to_owned)
    .to_vec()
}

impl Forwarder for SshForwarder {
    fn open(&self, host: &MachineLabel, port: u16) -> io::Result<Box<dyn Forward>> {
        let mut command = Command::new("ssh");
        command
            .args(forward_args(host, port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(windows)]
        end_stale_forwards(host, port);
        let mut child = command.spawn()?;
        #[cfg(windows)]
        if let Err(error) = super::windows_process::bind_to_app(&child) {
            eprintln!("toodo: the ssh forward to {host} may outlive the app: {error}");
        }
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("ssh has no piped standard error"));
        };
        let log: Arc<Mutex<Log>> = Arc::default();
        let reader_log = log.clone();
        let reader = thread::Builder::new()
            .name(format!("ssh-forward-{host}"))
            .spawn(move || read_log(stderr, &reader_log));
        let reader = match reader {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Box::new(SshForward {
            child,
            log,
            reader,
            exited_at: None,
        }))
    }
}

/// A process running on this machine, as far as finding stale forwards needs it.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    /// The executable's file name, e.g. `ssh.exe`.
    pub exe: String,
    /// Known only for processes that may be forwards.
    pub command_line: Option<String>,
    /// When the process started, comparable between processes; unknown if unreadable.
    pub started: Option<u64>,
}

/// Forwards to `host` that no running instance of the app (`app_exe`) owns: ssh with
/// exactly the arguments of `forward_args(host, port)` whose parent is gone, is another
/// program, or started after it (a reused process id). A forward whose start times are
/// unknown is kept, as is any ssh started with other arguments.
#[cfg(any(windows, test))]
pub fn stale_forwards(
    processes: &[Process],
    app_exe: &str,
    host: &MachineLabel,
    port: u16,
) -> Vec<u32> {
    let forward = forward_args(host, port);
    processes
        .iter()
        .filter(|process| {
            process
                .command_line
                .as_deref()
                .is_some_and(|line| is_ssh_with(line, &forward))
        })
        .filter(|forward| {
            !processes.iter().any(|parent| {
                parent.pid == forward.parent
                    && parent.exe.eq_ignore_ascii_case(app_exe)
                    && match (parent.started, forward.started) {
                        (Some(parent), Some(forward)) => parent <= forward,
                        _ => true,
                    }
            })
        })
        .map(|forward| forward.pid)
        .collect()
}

#[cfg(any(windows, test))]
fn is_ssh_with(command_line: &str, expected: &[String]) -> bool {
    let args = split_command_line(command_line);
    let Some((program, args)) = args.split_first() else {
        return false;
    };
    let name = program.rsplit(['\\', '/']).next().unwrap_or(program);
    (name.eq_ignore_ascii_case("ssh") || name.eq_ignore_ascii_case("ssh.exe")) && args == expected
}

/// Splits a Windows command line the way the C runtime does: the program name ends at
/// whitespace outside quotes; in arguments, `2n` backslashes before a quote are `n`
/// backslashes and the quote toggles quoting, `2n + 1` are `n` and a literal quote, and
/// `""` inside quotes is a literal quote.
#[cfg(any(windows, test))]
pub fn split_command_line(line: &str) -> Vec<String> {
    let is_blank = |c: &char| matches!(c, ' ' | '\t');
    let mut chars = line.chars().peekable();
    let mut args = Vec::new();
    let mut program = String::new();
    let mut quoted = false;
    while let Some(c) = chars.next_if(|c| quoted || !is_blank(c)) {
        if c == '"' {
            quoted = !quoted;
        } else {
            program.push(c);
        }
    }
    args.push(program);
    loop {
        while chars.next_if(is_blank).is_some() {}
        if chars.peek().is_none() {
            return args;
        }
        let mut arg = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            if is_blank(&c) && !quoted {
                break;
            }
            chars.next();
            match c {
                '\\' => {
                    let mut backslashes = 1;
                    while chars.next_if_eq(&'\\').is_some() {
                        backslashes += 1;
                    }
                    if chars.peek() == Some(&'"') {
                        arg.extend(std::iter::repeat_n('\\', backslashes / 2));
                        if backslashes % 2 == 1 {
                            chars.next();
                            arg.push('"');
                        }
                    } else {
                        arg.extend(std::iter::repeat_n('\\', backslashes));
                    }
                }
                '"' if quoted && chars.next_if_eq(&'"').is_some() => arg.push('"'),
                '"' => quoted = !quoted,
                c => arg.push(c),
            }
        }
        args.push(arg);
    }
}

/// Ends the forwards to `host` that an instance of the app which is gone left running,
/// so they no longer hold the remote port.
#[cfg(windows)]
fn end_stale_forwards(host: &MachineLabel, port: u16) {
    use super::windows_process as win;

    let app_exe = std::env::current_exe()
        .ok()
        .and_then(|path| Some(path.file_name()?.to_string_lossy().into_owned()));
    let Some(app_exe) = app_exe else { return };
    let mut processes = match win::processes() {
        Ok(processes) => processes,
        Err(error) => {
            eprintln!("toodo: could not look for leftover ssh forwards: {error}");
            return;
        }
    };
    for process in &mut processes {
        if process.exe.eq_ignore_ascii_case("ssh.exe") {
            process.command_line = win::command_line(process.pid).ok();
            process.started = win::started(process.pid).ok();
        } else if process.exe.eq_ignore_ascii_case(&app_exe) {
            process.started = win::started(process.pid).ok();
        }
    }
    for pid in stale_forwards(&processes, &app_exe, host, port) {
        match win::terminate(pid) {
            Ok(()) => eprintln!("toodo: ended a leftover ssh forward to {host} (pid {pid})"),
            Err(error) => {
                eprintln!(
                    "toodo: could not end the leftover ssh forward to {host} (pid {pid}): {error}"
                )
            }
        }
    }
}

/// What ssh reported so far.
#[derive(Debug, Default, PartialEq, Eq)]
struct Log {
    open: bool,
    /// The last line that may explain an exit.
    error: Option<String>,
}

impl Log {
    fn apply(&mut self, line: &str) {
        let line = line.trim();
        if line.contains("remote forward success for: listen") {
            self.open = true;
        } else if !line.is_empty() && !is_chatter(line) {
            self.error = Some(line.to_owned());
        }
    }
}

/// Lines ssh prints at `-v` that never explain why it exited.
fn is_chatter(line: &str) -> bool {
    [
        "debug",
        "OpenSSH_",
        "Authenticated to ",
        "Transferred: ",
        "Bytes per second: ",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

fn read_log(stderr: impl Read, log: &Mutex<Log>) {
    for line in BufReader::new(stderr).split(b'\n') {
        let Ok(line) = line else { break };
        log.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .apply(&String::from_utf8_lossy(&line));
    }
}

/// How long an exited ssh may take to finish printing why before it is reported.
const STDERR_GRACE: Duration = Duration::from_secs(2);

struct SshForward {
    child: Child,
    log: Arc<Mutex<Log>>,
    reader: JoinHandle<()>,
    exited_at: Option<Instant>,
}

impl Forward for SshForward {
    fn state(&mut self) -> ForwardState {
        let status = match self.child.try_wait() {
            Ok(status) => status,
            Err(error) => return ForwardState::Closed(format!("could not check ssh: {error}")),
        };
        let log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        let alive = if log.open {
            ForwardState::Open
        } else {
            ForwardState::Connecting
        };
        let Some(status) = status else {
            return alive;
        };
        let exited_at = *self.exited_at.get_or_insert_with(Instant::now);
        if !self.reader.is_finished() && exited_at.elapsed() < STDERR_GRACE {
            return alive;
        }
        ForwardState::Closed(
            log.error
                .clone()
                .unwrap_or_else(|| format!("ssh exited with {status}")),
        )
    }
}

impl Drop for SshForward {
    fn drop(&mut self) {
        // ssh may have exited already, in which case there is nothing to stop.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
pub mod fake {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A forward whose state the test sets; `alive` turns false once it is dropped.
    pub struct FakeForward {
        pub state: Arc<Mutex<ForwardState>>,
        pub alive: Arc<Mutex<bool>>,
    }

    impl Forward for FakeForward {
        fn state(&mut self) -> ForwardState {
            self.state.lock().unwrap().clone()
        }
    }

    impl Drop for FakeForward {
        fn drop(&mut self) {
            *self.alive.lock().unwrap() = false;
        }
    }

    /// Handles to one opened fake forward.
    #[derive(Clone)]
    pub struct Opened {
        pub host: MachineLabel,
        pub state: Arc<Mutex<ForwardState>>,
        pub alive: Arc<Mutex<bool>>,
    }

    /// Opens fake forwards, or fails with the scripted errors first.
    #[derive(Default)]
    pub struct FakeForwarder {
        pub failures: Mutex<VecDeque<io::Error>>,
        pub opened: Mutex<Vec<Opened>>,
    }

    impl Forwarder for Arc<FakeForwarder> {
        fn open(&self, host: &MachineLabel, _port: u16) -> io::Result<Box<dyn Forward>> {
            if let Some(error) = self.failures.lock().unwrap().pop_front() {
                return Err(error);
            }
            let opened = Opened {
                host: host.clone(),
                state: Arc::new(Mutex::new(ForwardState::Connecting)),
                alive: Arc::new(Mutex::new(true)),
            };
            self.opened.lock().unwrap().push(opened.clone());
            Ok(Box::new(FakeForward {
                state: opened.state,
                alive: opened.alive,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_the_same_loopback_port_over_a_dedicated_connection() {
        let args = forward_args(&MachineLabel::parse("devbox").unwrap(), 47615);
        assert_eq!(args[..2], ["-v", "-N"]);
        for option in [
            "BatchMode=yes",
            "ExitOnForwardFailure=yes",
            "ControlPath=none",
        ] {
            let at = args.iter().position(|arg| arg == option).unwrap();
            assert_eq!(args[at - 1], "-o");
        }
        assert_eq!(
            args[args.len() - 4..],
            ["-R", "127.0.0.1:47615:127.0.0.1:47615", "--", "devbox"]
        );
    }

    #[test]
    fn splits_windows_command_lines_like_the_c_runtime() {
        assert_eq!(
            split_command_line(
                r#""C:\Windows\System32\OpenSSH\ssh.exe" -v  -R 127.0.0.1:1:127.0.0.1:1	-- devbox"#
            ),
            [
                r"C:\Windows\System32\OpenSSH\ssh.exe",
                "-v",
                "-R",
                "127.0.0.1:1:127.0.0.1:1",
                "--",
                "devbox"
            ]
        );
        assert_eq!(
            split_command_line(r#"C:\a\b.exe "x y" a\\b c\"d "e\\" "" "f""g" h\\\"i"#),
            [
                r"C:\a\b.exe",
                "x y",
                r"a\\b",
                r#"c"d"#,
                r"e\",
                "",
                r#"f"g"#,
                r#"h\"i"#
            ]
        );
        assert_eq!(split_command_line("ssh"), ["ssh"]);
    }

    #[test]
    fn stale_forwards_are_exact_forwards_without_a_live_app_parent() {
        let devbox = MachineLabel::parse("devbox").unwrap();
        let line = |program: &str, host: &str, port: u16| {
            let host = MachineLabel::parse(host).unwrap();
            std::iter::once(program.to_owned())
                .chain(forward_args(&host, port))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let process = |pid, parent, exe: &str, command_line: Option<String>, started| Process {
            pid,
            parent,
            exe: exe.to_owned(),
            command_line,
            started,
        };
        let forward = line(r#""ssh""#, "devbox", 47615);
        let processes = [
            process(1, 0, "toodo.exe", None, Some(100)),
            process(2, 0, "explorer.exe", None, Some(50)),
            process(3, 0, "toodo.exe", None, Some(500)),
            process(4, 0, "toodo.exe", None, None),
            // Owned by the running app: kept.
            process(10, 1, "ssh.exe", Some(forward.clone()), Some(200)),
            // Its app is gone: ended.
            process(11, 99, "ssh.exe", Some(forward.clone()), Some(200)),
            // Its parent id now belongs to another program: ended.
            process(12, 2, "ssh.exe", Some(forward.clone()), Some(200)),
            // Its parent id now belongs to a later app instance: ended.
            process(13, 3, "ssh.exe", Some(forward.clone()), Some(200)),
            // An app parent whose start time is unknown may own it: kept.
            process(14, 4, "ssh.exe", Some(forward.clone()), Some(200)),
            process(15, 1, "ssh.exe", Some(forward.clone()), None),
            // Other machines, ports, programs and arguments: kept.
            process(
                20,
                99,
                "ssh.exe",
                Some(line("ssh", "gpu", 47615)),
                Some(200),
            ),
            process(
                21,
                99,
                "ssh.exe",
                Some(line("ssh", "devbox", 47616)),
                Some(200),
            ),
            process(
                22,
                99,
                "scp.exe",
                Some(line("scp", "devbox", 47615)),
                Some(200),
            ),
            process(23, 99, "ssh.exe", Some(format!("{forward} -x")), Some(200)),
            process(
                24,
                99,
                "ssh.exe",
                Some("ssh -N -R 127.0.0.1:47615:127.0.0.1:47615 -- devbox".to_owned()),
                Some(200),
            ),
            process(25, 99, "ssh.exe", None, Some(200)),
            // Any ssh path and case.
            process(
                26,
                99,
                "ssh.exe",
                Some(line(
                    r"C:\Windows\System32\OpenSSH\SSH.EXE",
                    "devbox",
                    47615,
                )),
                Some(200),
            ),
        ];
        assert_eq!(
            stale_forwards(&processes, "toodo.exe", &devbox, 47615),
            [11, 12, 13, 26]
        );
        // Another app executable owns none of them.
        assert_eq!(
            stale_forwards(&processes, "other.exe", &devbox, 47615),
            [10, 11, 12, 13, 14, 15, 26]
        );
    }

    #[test]
    fn the_log_tracks_the_forward_and_the_reason_ssh_exited() {
        let log = Mutex::new(Log::default());
        let stderr = "OpenSSH_9.6p1 Ubuntu-3ubuntu13, OpenSSL 3.0.13 30 Jan 2024\r\n\
            debug1: Connecting to devbox port 22.\n\
            Authenticated to devbox ([10.0.0.5]:22) using \"publickey\".\n\
            debug1: remote forward success for: listen 127.0.0.1:47615, connect 127.0.0.1:47615\n";
        read_log(stderr.as_bytes(), &log);
        assert_eq!(
            *log.lock().unwrap(),
            Log {
                open: true,
                error: None
            }
        );

        let log = Mutex::new(Log::default());
        let stderr = "debug1: Remote connections from 127.0.0.1:47615 forwarded to local address 127.0.0.1:47615\n\
            Error: remote port forwarding failed for listen port 47615\n\
            Transferred: sent 2580, received 2196 bytes, in 0.1 seconds\n\
            Bytes per second: sent 39147.2, received 33320.7\n";
        read_log(stderr.as_bytes(), &log);
        assert_eq!(
            *log.lock().unwrap(),
            Log {
                open: false,
                error: Some(
                    "Error: remote port forwarding failed for listen port 47615".to_owned()
                ),
            }
        );
    }
}
