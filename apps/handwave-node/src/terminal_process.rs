//! Native terminal child ownership for the product host. Terminal bytes are not
//! shell commands: the caller owns input authorization, logging and VT parsing.
use std::{io, process::ExitStatus};
use tokio::io::{AsyncRead, AsyncWrite};

pub type TerminalReader = Box<dyn AsyncRead + Send + Unpin>;
pub type TerminalWriter = Box<dyn AsyncWrite + Send + Unpin>;

#[cfg(any(unix, windows))]
pub use native::TerminalProcess;

fn dimensions(rows: u16, cols: u16) -> io::Result<()> {
    if rows == 0 || cols == 0 || rows > 32767 || cols > 32767 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "terminal dimensions must be 1..32767"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        process::Command,
    };

    fn python(code: &str) -> Command {
        let executable = std::env::var_os("HANDWAVE_TERMINAL_TEST_PYTHON").unwrap_or_else(|| {
            if cfg!(windows) {
                "python".into()
            } else {
                "python3".into()
            }
        });
        let mut command = Command::new(executable);
        command.args(["-u", "-c", code]);
        command
    }
    async fn wait(process: &mut TerminalProcess) -> ExitStatus {
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let Some(status) = process.try_wait().unwrap() {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("terminal process timed out")
    }
    async fn output(mut reader: TerminalReader) -> Vec<u8> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        bytes
    }
    #[tokio::test]
    async fn terminal_is_tty_input_environment_and_resize() {
        let mut command = python("import os,sys\nprint('TTY',sys.stdin.isatty(),sys.stdout.isatty(),sys.stderr.isatty(),flush=True)\nprint('ENV',os.environ.get('HANDWAVE_TERMINAL_TEST'),flush=True)\nprint('PROMPT',flush=True)\ns=input()\nt=os.get_terminal_size()\nprint('RESULT',repr(s),t.columns,t.lines,flush=True)");
        command.env("HANDWAVE_TERMINAL_TEST", "native-terminal");
        let mut process = TerminalProcess::spawn(&command, 24, 80).await.unwrap();
        let mut reader = process.take_reader().unwrap();
        let mut writer = process.take_writer().unwrap();
        let mut prefix = Vec::new();
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut buffer = [0u8; 4096];
            while !String::from_utf8_lossy(&prefix).contains("PROMPT") {
                let n = reader.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0, "child exited before prompt");
                prefix.extend_from_slice(&buffer[..n]);
            }
        })
        .await
        .unwrap();
        process.resize(37, 103).unwrap();
        writer.write_all(b"hello terminal\r").await.unwrap();
        let drain = tokio::spawn(output(reader));
        assert!(wait(&mut process).await.success());
        process.terminate_tree().unwrap();
        process.close_terminal();
        drop(writer);
        prefix.extend(tokio::time::timeout(Duration::from_secs(8), drain).await.unwrap().unwrap());
        let text = String::from_utf8_lossy(&prefix);
        assert!(text.contains("TTY True True True"), "{text}");
        assert!(text.contains("ENV native-terminal"), "{text}");
        assert!(text.contains("'hello terminal' 103 37"), "{text}");
    }
    #[tokio::test]
    async fn terminal_ctrl_c_interrupts_foreground_program() {
        let mut process = TerminalProcess::spawn(&python("import signal,time\nsignal.signal(signal.SIGINT,lambda *_:exit(23))\nprint('READY',flush=True)\ntime.sleep(120)"), 24, 80).await.unwrap();
        let mut reader = process.take_reader().unwrap();
        let mut writer = process.take_writer().unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut text = Vec::new();
            let mut b = [0u8; 1024];
            while !String::from_utf8_lossy(&text).contains("READY") {
                let n = reader.read(&mut b).await.unwrap();
                assert_ne!(n, 0);
                text.extend_from_slice(&b[..n]);
            }
        })
        .await
        .unwrap();
        let drain = tokio::spawn(output(reader));
        writer.write_all(&[3]).await.unwrap();
        assert_eq!(wait(&mut process).await.code(), Some(23));
        process.terminate_tree().unwrap();
        process.close_terminal();
        drop(writer);
        tokio::time::timeout(Duration::from_secs(8), drain).await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn terminal_large_output_cancel_drains_without_hanging_runtime() {
        let mut process = TerminalProcess::spawn(
            &python("import sys\nwhile True: sys.stdout.write('terminal-output-'*1000);sys.stdout.flush()"),
            24,
            80,
        )
        .await
        .unwrap();
        let reader = process.take_reader().unwrap();
        let drain = tokio::spawn(output(reader));
        for step in 0..40 {
            process.resize(24 + step % 8, 80 + step % 23).unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        process.terminate_tree().unwrap();
        wait(&mut process).await;
        process.close_terminal();
        tokio::time::timeout(Duration::from_secs(8), drain).await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn terminal_rejects_invalid_size_before_starting_child() {
        assert!(TerminalProcess::spawn(&python("raise SystemExit(99)"), 0, 80).await.is_err());
        assert!(TerminalProcess::spawn(&python("raise SystemExit(99)"), 24, 40000).await.is_err());
    }

    #[tokio::test]
    async fn terminal_cancel_owns_descendant_tree() {
        cancel_descendant_tree(python("import subprocess,sys,time\np=subprocess.Popen([sys.executable,'-c','import time;time.sleep(120)'])\nprint('DESCENDANT='+str(p.pid),flush=True)\ntime.sleep(120)")).await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn terminal_cancel_owns_shell_job_control_groups() {
        let mut command = Command::new("bash");
        command.args(["--noprofile", "--norc", "-m", "-c", "sleep 120 & echo DESCENDANT=$!; wait"]);
        cancel_descendant_tree(command).await;
    }
    async fn cancel_descendant_tree(command: Command) {
        let mut process = TerminalProcess::spawn(&command, 24, 80).await.unwrap();
        let mut reader = process.take_reader().unwrap();
        let descendant = tokio::time::timeout(Duration::from_secs(8), async {
            let mut text = String::new();
            let mut bytes = [0u8; 2048];
            loop {
                let n = reader.read(&mut bytes).await.unwrap();
                assert_ne!(n, 0);
                text.push_str(&String::from_utf8_lossy(&bytes[..n]));
                if let Some((_, tail)) = text.split_once("DESCENDANT=") {
                    let pid: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if tail.len() > pid.len() && !pid.is_empty() {
                        break pid.parse::<u32>().unwrap();
                    }
                }
            }
        })
        .await
        .unwrap();
        let drain = tokio::spawn(output(reader));
        process.terminate_tree().unwrap();
        wait(&mut process).await;
        process.close_terminal();
        tokio::time::timeout(Duration::from_secs(8), drain).await.unwrap().unwrap();
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, WAIT_OBJECT_0},
                System::Threading::{OpenProcess, WaitForSingleObject, SYNCHRONIZATION_SYNCHRONIZE},
            };
            let handle = OpenProcess(SYNCHRONIZATION_SYNCHRONIZE, 0, descendant);
            if !handle.is_null() {
                let state = WaitForSingleObject(handle, 3000);
                CloseHandle(handle);
                assert_eq!(state, WAIT_OBJECT_0, "descendant survived Job Object termination");
            }
        }
        #[cfg(unix)]
        {
            // A reparented killed child can briefly be a zombie before init
            // reaps it; that is terminated, not an executing descendant.
            let path = format!("/proc/{descendant}/stat");
            if let Ok(stat) = std::fs::read_to_string(path) {
                let state = stat.rsplit_once(") ").unwrap().1.chars().next().unwrap();
                assert_eq!(state, 'Z', "descendant survived process-group termination");
            }
        }
    }
}

#[cfg(unix)]
mod native {
    use super::*;
    use std::{
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        os::unix::process::CommandExt,
        pin::Pin,
        process::Stdio,
        task::{ready, Context, Poll},
    };
    use tokio::{
        io::{unix::AsyncFd, ReadBuf},
        process::{Child, Command},
    };

    struct PtyIo(AsyncFd<OwnedFd>);
    impl AsyncRead for PtyIo {
        fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            loop {
                let mut ready_guard = ready!(self.0.poll_read_ready(cx))?;
                let result = ready_guard.try_io(|inner| {
                    let dst = buf.initialize_unfilled();
                    let count = unsafe { libc::read(inner.get_ref().as_raw_fd(), dst.as_mut_ptr().cast(), dst.len()) };
                    if count >= 0 {
                        return Ok(count as usize);
                    }
                    let error = io::Error::last_os_error();
                    // Linux reports EIO after the last slave closes.
                    if error.raw_os_error() == Some(libc::EIO) {
                        Ok(0)
                    } else {
                        Err(error)
                    }
                });
                match result {
                    Ok(Ok(count)) => {
                        buf.advance(count);
                        return Poll::Ready(Ok(()));
                    }
                    Ok(Err(error)) => return Poll::Ready(Err(error)),
                    Err(_) => continue,
                }
            }
        }
    }
    impl AsyncWrite for PtyIo {
        fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
            loop {
                let mut guard = ready!(self.0.poll_write_ready(cx))?;
                match guard.try_io(|inner| {
                    let n = unsafe { libc::write(inner.get_ref().as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
                    if n < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(n as usize)
                    }
                }) {
                    Ok(result) => return Poll::Ready(result),
                    Err(_) => continue,
                }
            }
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    pub struct TerminalProcess {
        child: Child,
        pid: u32,
        terminated: bool,
        control: OwnedFd,
        reader: Option<TerminalReader>,
        writer: Option<TerminalWriter>,
    }
    impl TerminalProcess {
        pub async fn spawn(command: &Command, rows: u16, cols: u16) -> io::Result<Self> {
            dimensions(rows, cols)?;
            let mut master = -1;
            let mut slave = -1;
            let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
            if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &size) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let master = unsafe { OwnedFd::from_raw_fd(master) };
            let slave = unsafe { OwnedFd::from_raw_fd(slave) };
            // openpty descriptors are not CLOEXEC by default. In particular the
            // master must never survive exec in this child or its descendants.
            for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
                if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let source = command.as_std();
            let mut child_command = std::process::Command::new(source.get_program());
            child_command.args(source.get_args());
            for (key, value) in source.get_envs() {
                if let Some(value) = value {
                    child_command.env(key, value);
                } else {
                    child_command.env_remove(key);
                }
            }
            if let Some(cwd) = source.get_current_dir() {
                child_command.current_dir(cwd);
            }
            child_command.env("TERM", "xterm-256color");
            child_command
                .stdin(Stdio::from(slave.try_clone()?))
                .stdout(Stdio::from(slave.try_clone()?))
                .stderr(Stdio::from(slave.try_clone()?));
            unsafe {
                child_command.pre_exec(|| {
                    if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            // Each AsyncFd owns its descriptor; OwnedFd keeps that descriptor
            // open and unchanged until the registration itself is dropped.
            let read_fd = unsafe { AsyncFd::register(master.try_clone()?) }.map_err(|error| error.into_parts().1)?;
            let write_fd = unsafe { AsyncFd::register(master.try_clone()?) }.map_err(|error| error.into_parts().1)?;
            let child = Command::from(child_command).kill_on_drop(true).spawn()?;
            let pid = child.id().ok_or_else(|| io::Error::other("terminal child has no PID"))?;
            Ok(Self {
                child,
                pid,
                terminated: false,
                control: master,
                reader: Some(Box::new(PtyIo(read_fd))),
                writer: Some(Box::new(PtyIo(write_fd))),
            })
        }
        pub fn id(&self) -> u32 {
            self.pid
        }
        pub fn take_reader(&mut self) -> Option<TerminalReader> {
            self.reader.take()
        }
        pub fn take_writer(&mut self) -> Option<TerminalWriter> {
            self.writer.take()
        }
        pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.child.try_wait()
        }
        pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
            dimensions(rows, cols)?;
            let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
            if unsafe { libc::ioctl(self.control.as_raw_fd(), libc::TIOCSWINSZ as _, &size) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        pub fn terminate_tree(&mut self) -> io::Result<()> {
            if self.terminated {
                return Ok(());
            }
            // Do not kill a potentially reused process-group ID again during
            // Drop after an explicit termination and output-drain period.
            self.terminated = true;
            let mut groups = std::collections::BTreeSet::from([self.pid as i32]);
            let mut failure = None;
            let foreground = unsafe { libc::tcgetpgrp(self.control.as_raw_fd()) };
            if foreground > 0 && unsafe { libc::getsid(foreground) } == self.pid as i32 {
                groups.insert(foreground);
            }
            #[cfg(target_os = "linux")]
            {
                // A job-controlling shell creates additional process groups.
                // They remain ours while they are in the new terminal session.
                // Deliberate setsid()/daemon escape is not an OS sandbox.
                let entries = match std::fs::read_dir("/proc") {
                    Ok(entries) => Some(entries),
                    Err(error) => {
                        failure = Some(error);
                        None
                    }
                };
                for entry in entries.into_iter().flatten() {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            failure = Some(error);
                            continue;
                        }
                    };
                    let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                        continue;
                    };
                    let Some((_, fields)) = stat.rsplit_once(") ") else {
                        continue;
                    };
                    let fields: Vec<_> = fields.split_whitespace().take(4).collect();
                    if fields.len() == 4 && fields[3].parse::<u32>().ok() == Some(self.pid) {
                        if let Ok(group) = fields[2].parse::<i32>() {
                            if group > 0 {
                                groups.insert(group);
                            }
                        }
                    }
                }
            }
            for group in groups {
                if unsafe { libc::kill(-group, libc::SIGKILL) } < 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        failure = Some(error);
                    }
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            Ok(())
        }
        pub fn close_terminal(&mut self) {}
    }
    impl Drop for TerminalProcess {
        fn drop(&mut self) {
            let _ = self.terminate_tree();
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        collections::BTreeMap,
        ffi::{OsStr, OsString},
        mem::{size_of, zeroed},
        os::windows::{ffi::OsStrExt, process::ExitStatusExt},
        path::PathBuf,
        ptr::{null, null_mut},
    };
    use tokio::{
        net::windows::named_pipe::{NamedPipeServer, ServerOptions},
        process::Command,
    };
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Storage::FileSystem::{CreateFileW, OPEN_EXISTING},
        System::{
            Console::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON},
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::{
                CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, InitializeProcThreadAttributeList,
                ResumeThread, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_SUSPENDED,
                CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
            },
        },
    };

    struct Handle(HANDLE);
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    struct Console(HPCON, std::sync::mpsc::Sender<HPCON>);
    impl Console {
        unsafe fn create(input: HANDLE, output: HANDLE, rows: u16, cols: u16) -> io::Result<Self> {
            let (sender, receiver) = std::sync::mpsc::channel::<HPCON>();
            // Allocate the cleanup thread before creating the OS resource, so
            // thread exhaustion is a clean spawn error rather than a leaked HPC.
            std::thread::Builder::new().name("conpty-close".into()).spawn(move || {
                if let Ok(console) = receiver.recv() {
                    unsafe {
                        ClosePseudoConsole(console);
                    }
                }
            })?;
            let mut console = 0;
            let result = CreatePseudoConsole(COORD { X: cols as i16, Y: rows as i16 }, input, output, 0, &mut console);
            if result < 0 {
                return Err(io::Error::other(format!("CreatePseudoConsole HRESULT {result:#x}")));
            }
            Ok(Self(console, sender))
        }
    }
    impl Drop for Console {
        fn drop(&mut self) {
            // ConPTY close may wait for output consumption. Never block a Tokio
            // executor (especially the worker's current-thread runtime) here.
            // The receiver owns an already-created thread and only exits after
            // receiving this handle or observing spawn failure sender closure.
            let _ = self.1.send(self.0);
        }
    }
    struct Attributes(Vec<usize>);
    impl Attributes {
        fn pointer(&mut self) -> *mut std::ffi::c_void {
            self.0.as_mut_ptr().cast()
        }
    }
    impl Drop for Attributes {
        fn drop(&mut self) {
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }
    fn quote(value: &OsStr) -> Vec<u16> {
        // Windows CRT argv quoting, including quotes and trailing backslashes.
        let encoded: Vec<u16> = value.encode_wide().collect();
        if !encoded.is_empty() && !encoded.iter().any(|c| [b' ' as u16, b'\t' as u16, b'"' as u16].contains(c)) {
            return encoded;
        }
        let mut result = vec![b'"' as u16];
        let mut slashes = 0;
        for c in value.encode_wide() {
            if c == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            if c == b'"' as u16 {
                result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
            } else {
                result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
            }
            slashes = 0;
            result.push(c);
        }
        result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        result.push(b'"' as u16);
        result
    }
    fn environment(source: &std::process::Command) -> (Vec<u16>, BTreeMap<OsString, (OsString, OsString)>) {
        let mut vars = BTreeMap::new();
        for (key, value) in std::env::vars_os() {
            vars.insert(OsString::from(key.to_string_lossy().to_uppercase()), (key, value));
        }
        for (key, value) in source.get_envs() {
            let normalized = OsString::from(key.to_string_lossy().to_uppercase());
            if let Some(value) = value {
                vars.insert(normalized, (key.into(), value.into()));
            } else {
                vars.remove(&normalized);
            }
        }
        let mut block = Vec::new();
        for (key, value) in vars.values() {
            block.extend(key.encode_wide());
            block.push(b'=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        if block.len() == 1 {
            block.push(0);
        }
        (block, vars)
    }
    fn executable(
        source: &std::process::Command,
        vars: &BTreeMap<OsString, (OsString, OsString)>,
    ) -> io::Result<PathBuf> {
        let name = PathBuf::from(source.get_program());
        let names = if name.extension().is_none() {
            vec![name.with_extension("exe"), name.clone()]
        } else {
            vec![name.clone()]
        };
        let mut roots = vec![source.get_current_dir().map(PathBuf::from).unwrap_or(std::env::current_dir()?)];
        if let Some((_, path)) = vars.get(OsStr::new("PATH")) {
            roots.extend(std::env::split_paths(path));
        }
        for candidate in &names {
            if candidate.is_absolute() && candidate.is_file() {
                return Ok(candidate.clone());
            }
            for root in &roots {
                let full = root.join(candidate);
                if full.is_file() {
                    return Ok(full);
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, format!("terminal executable not found: {}", name.display())))
    }
    async fn pipe(output: bool) -> io::Result<(NamedPipeServer, Handle)> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let name = format!(r"\\.\pipe\handwave-terminal-{}-{suffix}", std::process::id());
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .access_inbound(output)
            .access_outbound(!output)
            .reject_remote_clients(true)
            .create(&name)?;
        let name = wide(OsStr::new(&name));
        let client = unsafe {
            CreateFileW(
                name.as_ptr(),
                if output { GENERIC_WRITE } else { GENERIC_READ },
                0,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if client == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let client = Handle(client);
        server.connect().await?;
        Ok((server, client))
    }

    pub struct TerminalProcess {
        process: Handle,
        job: Handle,
        console: Option<Console>,
        pid: u32,
        reader: Option<TerminalReader>,
        writer: Option<TerminalWriter>,
    }
    impl TerminalProcess {
        pub async fn spawn(command: &Command, rows: u16, cols: u16) -> io::Result<Self> {
            dimensions(rows, cols)?;
            let (input, console_input) = pipe(false).await?;
            let (output, console_output) = pipe(true).await?;
            let source = command.as_std();
            let (environment, vars) = environment(source);
            let application = wide(executable(source, &vars)?.as_os_str());
            let mut args = quote(source.get_program());
            for arg in source.get_args() {
                args.push(b' ' as u16);
                args.extend(quote(arg));
            }
            args.push(0);
            let directory = source.get_current_dir().map(|p| wide(p.as_os_str()));
            unsafe {
                let console = Console::create(console_input.0, console_output.0, rows, cols)?;
                drop(console_input);
                drop(console_output);
                let raw_job = CreateJobObjectW(null(), null());
                if raw_job.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = Handle(raw_job);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let mut size = 0;
                InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
                let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
                if InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut size) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut attributes = Attributes(storage);
                if UpdateProcThreadAttribute(
                    attributes.pointer(),
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    console.0 as *const _,
                    size_of::<HPCON>(),
                    null_mut(),
                    null(),
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let mut startup: STARTUPINFOEXW = zeroed();
                startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
                // Explicitly replace the worker's redirected stdio. Otherwise
                // Windows can preserve parent pipe/file handles over ConPTY.
                startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
                startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
                startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
                startup.lpAttributeList = attributes.pointer();
                let mut info: PROCESS_INFORMATION = zeroed();
                if CreateProcessW(
                    application.as_ptr(),
                    args.as_mut_ptr(),
                    null(),
                    null(),
                    0,
                    CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                    environment.as_ptr().cast(),
                    directory.as_ref().map_or(null(), |d| d.as_ptr()),
                    &startup.StartupInfo,
                    &mut info,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let process = Handle(info.hProcess);
                let thread = Handle(info.hThread);
                if AssignProcessToJobObject(job.0, process.0) == 0 {
                    let error = io::Error::last_os_error();
                    TerminateProcess(process.0, 1);
                    return Err(error);
                }
                if ResumeThread(thread.0) == u32::MAX {
                    let error = io::Error::last_os_error();
                    TerminateJobObject(job.0, 1);
                    return Err(error);
                }
                Ok(Self {
                    process,
                    job,
                    console: Some(console),
                    pid: info.dwProcessId,
                    reader: Some(Box::new(output)),
                    writer: Some(Box::new(input)),
                })
            }
        }
        pub fn id(&self) -> u32 {
            self.pid
        }
        pub fn take_reader(&mut self) -> Option<TerminalReader> {
            self.reader.take()
        }
        pub fn take_writer(&mut self) -> Option<TerminalWriter> {
            self.writer.take()
        }
        pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            match unsafe { WaitForSingleObject(self.process.0, 0) } {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(Some(ExitStatus::from_raw(code)))
                    }
                }
                _ => Err(io::Error::last_os_error()),
            }
        }
        pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
            dimensions(rows, cols)?;
            let console =
                self.console.as_ref().ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "terminal is closed"))?;
            let result = unsafe { ResizePseudoConsole(console.0, COORD { X: cols as i16, Y: rows as i16 }) };
            if result < 0 {
                Err(io::Error::other(format!("ResizePseudoConsole HRESULT {result:#x}")))
            } else {
                Ok(())
            }
        }
        pub fn terminate_tree(&mut self) -> io::Result<()> {
            if unsafe { TerminateJobObject(self.job.0, 1) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        /// Call after process-tree termination while the output pump still
        /// drains. ClosePseudoConsole completes on its dedicated close thread.
        pub fn close_terminal(&mut self) {
            drop(self.console.take());
        }
    }
    impl Drop for TerminalProcess {
        fn drop(&mut self) {
            let _ = self.terminate_tree();
        }
    }
}
