//! The process, socket and clock seams of the local service.
//!
//! Production runs fixed argument lists from absolute paths the shell chose
//! itself, discards standard error, and keeps at most `MAX_STDOUT` bytes of
//! standard output; nothing in an `Invocation` comes from the webview.
//! Tests script every answer (`fake`), so nothing is started, reached or
//! slept on the test host.
use std::{
    ffi::OsString,
    future::Future,
    io,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    time::{Duration, Instant},
};

use tokio::io::AsyncReadExt;

use super::super::client::PlaneClient;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// At most this much standard output is kept. `jetd core` prints one JSON
/// object of a few hundred bytes.
pub(crate) const MAX_STDOUT: usize = 64 * 1024;

/// One fixed command: an absolute program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invocation {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    /// Replaces `PATH` for the child; otherwise the app's environment is
    /// inherited unchanged.
    pub(crate) path_env: Option<OsString>,
    /// Keep standard output (`jetd core`); otherwise it goes to /dev/null,
    /// so a grandchild holding the pipe cannot stall the call.
    pub(crate) capture: bool,
}

impl Invocation {
    pub(crate) fn new<I, S>(program: impl Into<PathBuf>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            path_env: None,
            capture: false,
        }
    }

    pub(crate) fn capturing(mut self) -> Self {
        self.capture = true;
        self
    }

    pub(crate) fn with_path(mut self, path: impl Into<OsString>) -> Self {
        self.path_env = Some(path.into());
        self
    }

    /// The program and arguments as text, for tests.
    #[cfg(test)]
    pub(crate) fn argv(&self) -> Vec<String> {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(
                self.args
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned()),
            )
            .collect()
    }
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finished {
    /// `None` when a signal ended the process.
    pub(crate) code: Option<i32>,
    pub(crate) stdout: Vec<u8>,
}

impl Finished {
    pub(crate) fn succeeded(&self) -> bool {
        self.code == Some(0)
    }
}

pub(crate) trait Processes: Send + Sync {
    /// Runs `invocation` to completion, waiting at most `limit`. A command
    /// that outlives the limit, or prints more than `MAX_STDOUT`, is killed.
    fn run<'a>(
        &'a self,
        invocation: &'a Invocation,
        limit: Duration,
    ) -> BoxFuture<'a, io::Result<Finished>>;

    /// Starts a process that outlives this app: its own process group, no
    /// standard I/O, working directory `/`.
    fn spawn_detached(&self, invocation: &Invocation) -> io::Result<()>;
}

/// Production: `tokio::process` with the argument list unchanged.
pub(crate) struct SystemProcesses;

impl Processes for SystemProcesses {
    fn run<'a>(
        &'a self,
        invocation: &'a Invocation,
        limit: Duration,
    ) -> BoxFuture<'a, io::Result<Finished>> {
        Box::pin(async move {
            // ASVS 1.2.5: an argument array, never a shell string.
            let mut command = tokio::process::Command::new(&invocation.program);
            command
                .args(&invocation.args)
                .stdin(Stdio::null())
                .stdout(if invocation.capture {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                // Native error text never crosses to the webview; it is not
                // read at all.
                .stderr(Stdio::null())
                .kill_on_drop(true);
            environment(&mut command, invocation);
            let mut child = command.spawn()?;
            let stdout = child.stdout.take();
            let work = async {
                let mut bytes = Vec::new();
                if let Some(stdout) = stdout {
                    stdout
                        .take(MAX_STDOUT as u64 + 1)
                        .read_to_end(&mut bytes)
                        .await?;
                    if bytes.len() > MAX_STDOUT {
                        return Err(io::Error::from(io::ErrorKind::InvalidData));
                    }
                }
                let status = child.wait().await?;
                Ok(Finished {
                    code: status.code(),
                    stdout: bytes,
                })
            };
            // A late or oversized command is dropped here, which kills it.
            tokio::time::timeout(limit, work)
                .await
                .unwrap_or_else(|_| Err(io::Error::from(io::ErrorKind::TimedOut)))
        })
    }

    fn spawn_detached(&self, invocation: &Invocation) -> io::Result<()> {
        let mut command = tokio::process::Command::new(&invocation.program);
        command
            .args(&invocation.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir("/")
            // A process group of its own: closing the app or its terminal
            // does not signal the daemon.
            .process_group(0)
            .kill_on_drop(false);
        environment(&mut command, invocation);
        // Tokio reaps the child in the background once it exits.
        command.spawn().map(drop)
    }
}

/// The app's environment for a child, without what the AppImage runtime
/// added, then the invocation's own `PATH`.
fn environment(command: &mut tokio::process::Command, invocation: &Invocation) {
    if let Some(appdir) = std::env::var_os("APPDIR") {
        for (name, value) in appimage_scrub(std::env::vars_os(), &appdir) {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
    }
    if let Some(path) = &invocation.path_env {
        command.env("PATH", path);
    }
}

/// The variables an AppImage's runtime and hooks set (`APPDIR`, `APPIMAGE`,
/// GTK and GIO module paths into its mount, entries prepended to search
/// paths). A daemon started from here outlives that mount, and the tools it
/// runs must see the session's own environment: variables naming the
/// AppImage are removed, and entries under `appdir` are dropped from lists.
pub(crate) fn appimage_scrub(
    variables: impl IntoIterator<Item = (OsString, OsString)>,
    appdir: &std::ffi::OsStr,
) -> Vec<(OsString, Option<OsString>)> {
    use std::os::unix::ffi::OsStrExt;
    let appdir = appdir.as_bytes();
    if appdir.len() < 2 || appdir[0] != b'/' {
        return Vec::new();
    }
    let inside = |entry: &[u8]| {
        entry.starts_with(appdir) && matches!(entry.get(appdir.len()), None | Some(b'/'))
    };
    let mut changes = Vec::new();
    for (name, value) in variables {
        if matches!(name.as_bytes(), b"APPDIR" | b"APPIMAGE" | b"ARGV0" | b"OWD") {
            changes.push((name, None));
            continue;
        }
        let bytes = value.as_bytes();
        if !bytes.windows(appdir.len()).any(|window| window == appdir) {
            continue;
        }
        let kept: Vec<&[u8]> = bytes
            .split(|byte| *byte == b':')
            .filter(|entry| !entry.is_empty() && !inside(entry))
            .collect();
        let replacement =
            (!kept.is_empty()).then(|| std::ffi::OsStr::from_bytes(&kept.join(&b':')).to_owned());
        changes.push((name, replacement));
    }
    changes
}

/// Whether the local Plane answers a `status` request. The local handshake
/// and request have no deadline of their own; callers bound each probe
/// (`Clock::within`).
pub(crate) trait Reachability: Send + Sync {
    fn reachable(&self) -> BoxFuture<'_, bool>;
}

/// Production: the bridge's own local `PlaneClient`, so the service check
/// uses exactly the transport every other local request uses.
pub(crate) struct LocalPlane(pub(crate) PlaneClient);

impl Reachability for LocalPlane {
    fn reachable(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move { self.0.status().await.is_ok() })
    }
}

pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()>;
    /// `work`'s answer, or `None` once `limit` passes first; the work is
    /// then dropped.
    fn within<'a>(
        &'a self,
        limit: Duration,
        work: BoxFuture<'a, bool>,
    ) -> BoxFuture<'a, Option<bool>>;
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }

    fn within<'a>(
        &'a self,
        limit: Duration,
        work: BoxFuture<'a, bool>,
    ) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async move { tokio::time::timeout(limit, work).await.ok() })
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! Scripted processes, socket and clock. The handler plays every
    //! program; the clock only moves when something sleeps on it.
    use std::{
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Mutex,
        },
        task::Poll,
    };

    use super::*;

    type Handler = Box<dyn Fn(&Invocation) -> io::Result<Finished> + Send + Sync>;
    type SpawnHandler = Box<dyn Fn(&Invocation) + Send + Sync>;

    pub(crate) struct FakeProcesses {
        handler: Handler,
        on_spawn: SpawnHandler,
        pub(crate) calls: Mutex<Vec<Invocation>>,
        pub(crate) detached: Mutex<Vec<Invocation>>,
        pub(crate) spawn_fails: AtomicBool,
    }

    impl FakeProcesses {
        pub(crate) fn new(
            handler: impl Fn(&Invocation) -> io::Result<Finished> + Send + Sync + 'static,
        ) -> Self {
            Self {
                handler: Box::new(handler),
                on_spawn: Box::new(|_| ()),
                calls: Mutex::new(Vec::new()),
                detached: Mutex::new(Vec::new()),
                spawn_fails: AtomicBool::new(false),
            }
        }

        /// Plays a detached start (for example, the daemon coming up).
        pub(crate) fn on_spawn(
            mut self,
            handler: impl Fn(&Invocation) + Send + Sync + 'static,
        ) -> Self {
            self.on_spawn = Box::new(handler);
            self
        }

        /// Every run, as argument lists, in order.
        pub(crate) fn argvs(&self) -> Vec<Vec<String>> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(Invocation::argv)
                .collect()
        }

        pub(crate) fn detached_argvs(&self) -> Vec<Vec<String>> {
            self.detached
                .lock()
                .unwrap()
                .iter()
                .map(Invocation::argv)
                .collect()
        }
    }

    impl Processes for FakeProcesses {
        fn run<'a>(
            &'a self,
            invocation: &'a Invocation,
            _limit: Duration,
        ) -> BoxFuture<'a, io::Result<Finished>> {
            self.calls.lock().unwrap().push(invocation.clone());
            let result = (self.handler)(invocation);
            Box::pin(async move { result })
        }

        fn spawn_detached(&self, invocation: &Invocation) -> io::Result<()> {
            if self.spawn_fails.load(Ordering::SeqCst) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.detached.lock().unwrap().push(invocation.clone());
            (self.on_spawn)(invocation);
            Ok(())
        }
    }

    /// Answers every probe with the socket's current state, or never.
    pub(crate) struct FakeReachability {
        reachable: AtomicBool,
        /// A daemon that accepts but never answers (stopped, wedged).
        hangs: AtomicBool,
        pub(crate) probes: AtomicUsize,
    }

    impl FakeReachability {
        pub(crate) fn new(reachable: bool) -> Self {
            Self {
                reachable: AtomicBool::new(reachable),
                hangs: AtomicBool::new(false),
                probes: AtomicUsize::new(0),
            }
        }

        pub(crate) fn set(&self, reachable: bool) {
            self.reachable.store(reachable, Ordering::SeqCst);
        }

        pub(crate) fn hang(&self, hangs: bool) {
            self.hangs.store(hangs, Ordering::SeqCst);
        }
    }

    impl Reachability for FakeReachability {
        fn reachable(&self) -> BoxFuture<'_, bool> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            if self.hangs.load(Ordering::SeqCst) {
                return Box::pin(std::future::pending());
            }
            let answer = self.reachable.load(Ordering::SeqCst);
            Box::pin(async move { answer })
        }
    }

    /// Virtual time: `sleep` advances `now` at once.
    pub(crate) struct FakeClock {
        now: Mutex<Instant>,
        pub(crate) slept: Mutex<Duration>,
    }

    impl FakeClock {
        pub(crate) fn new() -> Self {
            Self {
                now: Mutex::new(Instant::now()),
                slept: Mutex::new(Duration::ZERO),
            }
        }

        pub(crate) fn advance(&self, duration: Duration) {
            *self.now.lock().unwrap() += duration;
        }

        pub(crate) fn slept(&self) -> Duration {
            *self.slept.lock().unwrap()
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }

        fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()> {
            self.advance(duration);
            *self.slept.lock().unwrap() += duration;
            Box::pin(async {
                tokio::task::yield_now().await;
            })
        }

        /// Work that is not ready at its first poll never will be (every
        /// fake answers at once or never): `limit` passes at once.
        fn within<'a>(
            &'a self,
            limit: Duration,
            mut work: BoxFuture<'a, bool>,
        ) -> BoxFuture<'a, Option<bool>> {
            Box::pin(async move {
                let first =
                    std::future::poll_fn(|context| Poll::Ready(work.as_mut().poll(context))).await;
                match first {
                    Poll::Ready(answer) => Some(answer),
                    Poll::Pending => {
                        self.advance(limit);
                        None
                    }
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appimage_variables_do_not_reach_children() {
        let variables = [
            ("APPDIR", "/tmp/.mount_JetAb12"),
            ("APPIMAGE", "/home/u/Applications/Jet.AppImage"),
            ("OWD", "/home/u"),
            ("HOME", "/home/u"),
            (
                "GDK_PIXBUF_MODULE_FILE",
                "/tmp/.mount_JetAb12/usr/lib/loaders.cache",
            ),
            (
                "XDG_DATA_DIRS",
                "/tmp/.mount_JetAb12/usr/share:/usr/local/share:/usr/share",
            ),
            ("LD_LIBRARY_PATH", "/tmp/.mount_JetAb12/usr/lib"),
            ("PATH", "/tmp/.mount_JetAb12/usr/bin:/usr/bin"),
            ("NEIGHBOUR", "/tmp/.mount_JetAb123/usr/lib"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let changes = appimage_scrub(variables, std::ffi::OsStr::new("/tmp/.mount_JetAb12"));
        let expected: Vec<(OsString, Option<OsString>)> = [
            ("APPDIR", None),
            ("APPIMAGE", None),
            ("OWD", None),
            ("GDK_PIXBUF_MODULE_FILE", None),
            ("XDG_DATA_DIRS", Some("/usr/local/share:/usr/share")),
            ("LD_LIBRARY_PATH", None),
            ("PATH", Some("/usr/bin")),
            // Another mount that only shares the prefix is kept.
            ("NEIGHBOUR", Some("/tmp/.mount_JetAb123/usr/lib")),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), value.map(OsString::from)))
        .collect();
        assert_eq!(changes, expected);
        assert!(appimage_scrub(
            [(OsString::from("A"), OsString::from("/x"))],
            std::ffi::OsStr::new("relative")
        )
        .is_empty());
    }
}
