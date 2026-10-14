//! PTY channels: a `portable-pty` child bridged to an enzarb-proto channel.
//!
//! Data flow: PTY master -> reader thread (runs the [`PtyObserver`]) -> mpsc
//! -> channel (flow-controlled, so a slow client backpressures the process);
//! channel -> mpsc -> writer thread -> PTY master. When the process exits the
//! remaining output is drained and the channel is closed (`CLOSE`, code 0,
//! reason `exit:<code>` or `signal:<n>`).

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use enzarb_proto::close_code;
use enzarb_proto::session::ChannelHandle;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{mpsc, oneshot};

use crate::observer::PtyObserver;
use crate::reaper::{ExitStatus, Reaper};

/// After the process exits, how long output may stay idle before we stop
/// waiting for it (background children can keep the terminal open).
const DRAIN_IDLE: Duration = Duration::from_millis(300);

/// Where and how PTY processes start.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    pub shell: String,
    pub workspace: PathBuf,
}

type Master = Arc<Mutex<Box<dyn MasterPty + Send>>>;

/// Handle used to resize a running PTY.
#[derive(Clone)]
pub struct PtyControl {
    master: Master,
}

impl PtyControl {
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.master
            .lock()
            .unwrap()
            .resize(size(cols, rows))
            .map_err(io::Error::other)
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// A spawned PTY process. Dropping it before the process exited kills the
/// process group.
pub struct Pty {
    pid: u32,
    control: PtyControl,
    output: mpsc::Receiver<Bytes>,
    input: mpsc::Sender<Bytes>,
    exit: oneshot::Receiver<ExitStatus>,
    exited: bool,
}

impl Pty {
    /// Spawn `command` (or a login shell) in the workspace on a new PTY.
    pub fn spawn(
        reaper: &Reaper,
        cfg: &PtyConfig,
        cols: u16,
        rows: u16,
        command: Option<&[String]>,
        mut observer: Option<Box<dyn PtyObserver>>,
    ) -> io::Result<Pty> {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(io::Error::other)?;
        let argv: Vec<String> = match command {
            Some(c) if !c.is_empty() => c.to_vec(),
            _ => vec![cfg.shell.clone(), "-l".into()],
        };
        // As root, the child goes through sandbox-init's drop-privileges exec mode.
        let argv = crate::privs::wrap_pty_command(argv);
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        if cfg.workspace.is_dir() {
            cmd.cwd(&cfg.workspace);
        }
        for (k, v) in crate::privs::pty_env() {
            cmd.env(k, v);
        }
        cmd.env("TERM", "xterm-256color");
        let slave = pair.slave;
        let (child, exit) = reaper.spawn_with(|| {
            let child = slave.spawn_command(cmd).map_err(io::Error::other)?;
            let pid = child
                .process_id()
                .ok_or_else(|| io::Error::other("spawned child has no pid"))?;
            Ok((child, pid))
        })?;
        let pid = child.process_id().unwrap_or_default();
        // The reaper owns waiting; the portable-pty child handle is only a pid holder.
        drop(slave);
        drop(child);

        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let mut writer = pair.master.take_writer().map_err(io::Error::other)?;
        let master: Master = Arc::new(Mutex::new(pair.master));

        let (out_tx, output) = mpsc::channel::<Bytes>(16);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Some(o) = observer.as_mut() {
                            o.on_output(&buf[..n]);
                        }
                        if out_tx
                            .blocking_send(Bytes::copy_from_slice(&buf[..n]))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            if let Some(o) = observer.as_mut() {
                o.on_close();
            }
        });

        let (input, mut in_rx) = mpsc::channel::<Bytes>(16);
        std::thread::spawn(move || {
            while let Some(b) = in_rx.blocking_recv() {
                if writer.write_all(&b).and_then(|()| writer.flush()).is_err() {
                    break;
                }
            }
        });

        Ok(Pty {
            pid,
            control: PtyControl { master },
            output,
            input,
            exit,
            exited: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn control(&self) -> PtyControl {
        self.control.clone()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if !self.exited {
            let _ = killpg(Pid::from_raw(self.pid as i32), Signal::SIGKILL);
        }
    }
}

fn close_reason(st: Option<ExitStatus>) -> String {
    match st {
        Some(ExitStatus::Exited(c)) => format!("exit:{c}"),
        Some(ExitStatus::Signaled(s)) => format!("signal:{s}"),
        None => "exit:unknown".into(),
    }
}

/// Bridge `pty` to `channel` until the process exits (then drain and send
/// `CLOSE`), the channel's peer goes away, or the session ends. Dropping the
/// returned future (task abort) kills the process.
pub async fn bridge(channel: ChannelHandle, mut pty: Pty) {
    let (mut send, mut recv) = channel.split();
    let input = pty.input.clone();
    let in_pump = tokio::spawn(async move {
        while let Some(data) = recv.recv().await {
            if input.send(data.into_bytes()).await.is_err() {
                break;
            }
        }
        // Peer half-closed: keep the process running, stop forwarding.
        std::future::pending::<()>().await;
    });

    let mut status: Option<ExitStatus> = None;
    let mut exited = false;
    loop {
        let idle = async {
            if exited {
                tokio::time::sleep(DRAIN_IDLE).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            chunk = pty.output.recv() => match chunk {
                Some(b) => {
                    if send.send(b).await.is_err() {
                        break; // session gone or peer aborted the channel
                    }
                }
                None => break, // terminal closed
            },
            st = &mut pty.exit, if !exited => {
                exited = true;
                pty.exited = true;
                status = st.ok();
            }
            () = idle => break,
        }
    }
    in_pump.abort();
    if !exited {
        // Output ended before we saw the exit; give the reaper a moment.
        if let Ok(Ok(st)) = tokio::time::timeout(Duration::from_secs(2), &mut pty.exit).await {
            pty.exited = true;
            status = Some(st);
        }
    }
    let _ = send.close(close_code::NORMAL, &close_reason(status));
}
