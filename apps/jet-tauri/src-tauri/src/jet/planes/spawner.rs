//! The only way the shell reaches a remote Plane: the system `ssh` running
//! `jetd connect --stdio` on the target, over its standard I/O.
//!
//! The argument list is fixed by `SshEndpoint::command()` in `jet-client`:
//! strict host-key checking, batch mode, a connect timeout, no forwarding
//! and `--` before the destination. The shell adds nothing, never reads,
//! logs or forwards ssh's stderr (it goes to `/dev/null`), and classifies a
//! failure only by the exit status (`remote::classify`).
use std::{future::Future, io, pin::Pin, process::Stdio, time::Duration};

use jet_client::SshEndpoint;
use tokio::io::{AsyncRead, AsyncWrite};

pub(crate) type ExitFuture<'a> = Pin<Box<dyn Future<Output = Option<i32>> + Send + 'a>>;

/// The ssh process behind one connection. Dropping it kills the process.
pub(crate) trait SshChild: Send {
    /// Whether the process has already exited (the relay is gone).
    fn has_exited(&mut self) -> bool;
    /// The exit code, waiting at most `limit`. `None` while still running or
    /// when killed by a signal.
    fn wait_exit(&mut self, limit: Duration) -> ExitFuture<'_>;
}

pub(crate) struct SpawnedSsh {
    pub(crate) read: Box<dyn AsyncRead + Unpin + Send>,
    pub(crate) write: Box<dyn AsyncWrite + Unpin + Send>,
    pub(crate) child: Box<dyn SshChild>,
}

pub(crate) trait SshSpawner: Send + Sync {
    fn spawn(&self, endpoint: &SshEndpoint) -> io::Result<SpawnedSsh>;
}

/// Production spawner: the system OpenSSH client.
pub(crate) struct SystemSsh;

struct SystemChild(tokio::process::Child);

impl SshChild for SystemChild {
    fn has_exited(&mut self) -> bool {
        !matches!(self.0.try_wait(), Ok(None))
    }

    fn wait_exit(&mut self, limit: Duration) -> ExitFuture<'_> {
        Box::pin(async move {
            match tokio::time::timeout(limit, self.0.wait()).await {
                Ok(Ok(status)) => status.code(),
                Ok(Err(_)) | Err(_) => None,
            }
        })
    }
}

impl SshSpawner for SystemSsh {
    fn spawn(&self, endpoint: &SshEndpoint) -> io::Result<SpawnedSsh> {
        // ASVS 1.2.5: the argument array comes from jet-client unchanged.
        let mut command = tokio::process::Command::from(endpoint.command());
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let read = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        let write = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(SpawnedSsh {
            read: Box::new(read),
            write: Box::new(write),
            child: Box::new(SystemChild(child)),
        })
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted spawner: every spawn pops one server script and runs it on
    //! the far end of an in-memory duplex pipe. No process, no network.
    use std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    use tokio::{io::DuplexStream, sync::watch};

    use super::*;

    pub(crate) type Script =
        Box<dyn FnOnce(DuplexStream) -> Pin<Box<dyn Future<Output = Option<i32>> + Send>> + Send>;

    #[derive(Default)]
    pub(crate) struct FakeSpawner {
        scripts: Mutex<VecDeque<Script>>,
        spawns: AtomicUsize,
    }

    impl FakeSpawner {
        /// Queues the server side of the next spawned ssh. The script's
        /// return value is ssh's exit code.
        pub(crate) fn push<F, Fut>(&self, script: F)
        where
            F: FnOnce(DuplexStream) -> Fut + Send + 'static,
            Fut: Future<Output = Option<i32>> + Send + 'static,
        {
            self.scripts
                .lock()
                .unwrap()
                .push_back(Box::new(move |stream| Box::pin(script(stream))));
        }

        pub(crate) fn spawns(&self) -> usize {
            self.spawns.load(Ordering::SeqCst)
        }

        pub(crate) fn pending(&self) -> usize {
            self.scripts.lock().unwrap().len()
        }
    }

    struct FakeChild(watch::Receiver<Option<Option<i32>>>);

    impl SshChild for FakeChild {
        fn has_exited(&mut self) -> bool {
            self.0.borrow().is_some()
        }

        fn wait_exit(&mut self, limit: Duration) -> ExitFuture<'_> {
            Box::pin(async move {
                let exited = tokio::time::timeout(limit, self.0.wait_for(Option::is_some)).await;
                match exited {
                    Ok(Ok(code)) => code.flatten(),
                    _ => None,
                }
            })
        }
    }

    impl SshSpawner for FakeSpawner {
        fn spawn(&self, _endpoint: &SshEndpoint) -> io::Result<SpawnedSsh> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            let script = self
                .scripts
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no scripted ssh"))?;
            let (client, server) = tokio::io::duplex(1 << 20);
            let (exit, exited) = watch::channel(None);
            tokio::spawn(async move {
                let code = script(server).await;
                let _ = exit.send(Some(code));
            });
            let (read, write) = tokio::io::split(client);
            Ok(SpawnedSsh {
                read: Box::new(read),
                write: Box::new(write),
                child: Box::new(FakeChild(exited)),
            })
        }
    }
}
