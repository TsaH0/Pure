//! Background programs requested by sandboxed plugins after explicit grants.
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use helix_event::runtime_local;
use helix_plugin::{Callback, PendingKind, ProcessOutput, SpawnOptions};
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::time::{sleep_until, Instant};

pub(crate) const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

type RunningProcesses = HashMap<(String, u32), oneshot::Sender<()>>;

runtime_local! {
    static RUNNING: LazyLock<Mutex<RunningProcesses>> = LazyLock::new(|| Mutex::new(HashMap::new()));
}

pub(crate) fn start(plugin: String, id: u32, argv: Vec<String>, options: SpawnOptions) {
    if !crate::plugins::with_host(|host| host.request_is_pending(&plugin, id, PendingKind::Process))
        .unwrap_or(false)
    {
        return;
    }
    let (cancel, cancelled) = oneshot::channel();
    let cwd = helix_stdx::env::current_working_dir();
    RUNNING.lock().insert((plugin.clone(), id), cancel);
    tokio::spawn(async move {
        let result = run(argv, options, cwd, cancelled).await;
        RUNNING.lock().remove(&(plugin.clone(), id));
        crate::job::dispatch_with_jobs(move |editor, _, jobs| {
            crate::plugins::deliver_callback(
                editor,
                jobs,
                &plugin,
                Callback::ProcessDone { id, result },
            );
        })
        .await;
    });
}

pub(crate) fn kill(plugin: &str, id: u32) {
    if let Some(cancel) = RUNNING.lock().remove(&(plugin.to_owned(), id)) {
        let _ = cancel.send(());
    }
}

pub(crate) fn cancel_plugin(plugin: &str) {
    RUNNING.lock().retain(|(name, _), _| name != plugin);
}

pub(crate) fn cancel_all() {
    RUNNING.lock().clear();
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Capture {
    fn append(&mut self, bytes: &[u8]) {
        let remaining = OUTPUT_LIMIT.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        self.truncated |= bytes.len() > remaining;
    }

    fn text(&self) -> (String, bool) {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        let truncated = self.truncated || text.len() > OUTPUT_LIMIT;
        truncate(&mut text, OUTPUT_LIMIT);
        (text, truncated)
    }
}

fn truncate(text: &mut String, limit: usize) {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

async fn read_stream(
    mut stream: impl AsyncRead + Unpin,
    capture: Arc<Mutex<Capture>>,
) -> io::Result<()> {
    let mut chunk = [0; 16 * 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        capture.lock().append(&chunk[..read]);
    }
}

struct ProcessGroup {
    pid: Option<u32>,
    armed: bool,
}

impl ProcessGroup {
    fn kill(&self) {
        let pid = self.pid;
        #[cfg(unix)]
        if self.armed {
            if let Some(pid) = pid {
                // SAFETY: the child was started in its own process group.
                // Cancellation must also close pipes inherited by its children.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        let _ = (pid, self.armed);
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

enum Exit {
    Done(io::Result<ExitStatus>),
    Killed,
    TimedOut,
}

async fn run(
    argv: Vec<String>,
    options: SpawnOptions,
    cwd: PathBuf,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<ProcessOutput, String> {
    let program = argv.first().ok_or("empty process argv")?;
    let mut command = Command::new(program);
    command.args(&argv[1..]);
    command.current_dir(options.cwd.as_ref().map(PathBuf::from).unwrap_or(cwd));
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to run {program:?}: {err}"))?;
    let mut group = ProcessGroup {
        pid: child.id(),
        armed: true,
    };
    let stdout = Arc::new(Mutex::new(Capture::default()));
    let stderr = Arc::new(Mutex::new(Capture::default()));
    let mut read_out = tokio::spawn(read_stream(child.stdout.take().unwrap(), stdout.clone()));
    let mut read_err = tokio::spawn(read_stream(child.stderr.take().unwrap(), stderr.clone()));
    let mut stdin = child.stdin.take().unwrap();
    let mut write_in = tokio::spawn(async move {
        if let Some(input) = options.stdin {
            match stdin.write_all(input.as_bytes()).await {
                Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {}
                result => result?,
            }
        }
        Ok::<_, io::Error>(())
    });
    let deadline = Instant::now()
        + Duration::from_millis(options.timeout_ms.unwrap_or(30_000).min(600_000) as u64);
    let mut exit = tokio::select! {
        status = child.wait() => Exit::Done(status),
        _ = sleep_until(deadline) => Exit::TimedOut,
        _ = &mut cancelled => Exit::Killed,
    };
    if let Exit::Done(Err(err)) = exit {
        group.kill();
        group.armed = false;
        read_out.abort();
        read_err.abort();
        write_in.abort();
        return Err(format!("failed waiting for {program:?}: {err}"));
    }
    let mut io_error = None;
    if matches!(exit, Exit::Done(_)) {
        let streams = async { tokio::join!(&mut read_out, &mut read_err, &mut write_in) };
        tokio::select! {
            (out, err, input) = streams => {
                for result in [out, err, input] {
                    if let Err(err) = result.map_err(io::Error::other).and_then(|result| result) { io_error = Some(err); }
                }
            },
            _ = sleep_until(deadline) => exit = Exit::TimedOut,
            _ = &mut cancelled => exit = Exit::Killed,
        }
    }
    let (exit_code, reason) = match exit {
        Exit::Done(status) => {
            let status = status.map_err(|err| format!("failed waiting for {program:?}: {err}"))?;
            group.armed = false;
            (status.code(), None)
        }
        Exit::Killed | Exit::TimedOut => {
            group.kill();
            group.armed = false;
            let _ = child.kill().await;
            let _ = child.wait().await;
            read_out.abort();
            read_err.abort();
            write_in.abort();
            (
                None,
                Some(if matches!(exit, Exit::TimedOut) {
                    "process timed out"
                } else {
                    "process killed"
                }),
            )
        }
    };
    let (stdout, out_truncated) = stdout.lock().text();
    let (mut stderr, err_truncated) = stderr.lock().text();
    let mut notes = String::new();
    if out_truncated || err_truncated {
        notes.push_str("\n[process output truncated at 16 MiB per stream]\n");
    }
    if let Some(reason) = reason {
        notes.push_str(&format!("\n{reason}\n"));
    }
    if let Some(err) = io_error {
        notes.push_str(&format!("\nprocess I/O: {err}\n"));
    }
    truncate(&mut notes, OUTPUT_LIMIT);
    truncate(&mut stderr, OUTPUT_LIMIT.saturating_sub(notes.len()));
    stderr.push_str(&notes);
    Ok(ProcessOutput {
        exit_code,
        stdout,
        stderr,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn options(stdin: Option<String>, timeout_ms: u32) -> SpawnOptions {
        SpawnOptions {
            cwd: None,
            stdin,
            timeout_ms: Some(timeout_ms),
        }
    }

    async fn command(argv: &[&str], options: SpawnOptions) -> ProcessOutput {
        let (_cancel, receiver) = oneshot::channel();
        run(
            argv.iter().map(|arg| (*arg).into()).collect(),
            options,
            helix_stdx::env::current_working_dir(),
            receiver,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn captures_exit_code_and_both_streams_without_a_shell_wrapper() {
        let result = command(
            &["sh", "-c", "echo hi; echo err >&2; exit 3"],
            options(None, 5_000),
        )
        .await;
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(result.stdout, "hi\n");
        assert_eq!(result.stderr, "err\n");
        let result = command(
            &["printf", "%s", "$HOME; echo not-a-command"],
            options(None, 5_000),
        )
        .await;
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "$HOME; echo not-a-command");
    }

    #[tokio::test]
    async fn stdin_is_written_concurrently_and_closed() {
        let input = "héllo stdin\n".repeat(10_000);
        let result = command(&["cat"], options(Some(input.clone()), 5_000)).await;
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, input);
        assert!(result.stderr.is_empty());
    }

    #[tokio::test]
    async fn cwd_and_lossy_utf8_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(None, 5_000);
        opts.cwd = Some(dir.path().to_str().unwrap().into());
        let result = command(&["pwd"], opts).await;
        assert_eq!(result.stdout.trim(), dir.path().to_str().unwrap());
        let result = command(&["sh", "-c", "printf '\\377\\376'"], options(None, 5_000)).await;
        assert_eq!(result.stdout, "\u{fffd}\u{fffd}");
    }

    #[tokio::test]
    async fn timeout_includes_inherited_pipes_after_the_parent_exits() {
        let start = Instant::now();
        let result = command(&["sh", "-c", "sleep 5 &"], options(None, 50)).await;
        assert_eq!(result.exit_code, None);
        assert!(result.stderr.contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn cancellation_reaps_the_child_and_still_produces_a_result() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let (cancel, receiver) = oneshot::channel();
        let argv = vec![
            "sh".into(),
            "-c".into(),
            "printf '%s' $$ > \"$1\"; exec sleep 5".into(),
            "sh".into(),
            pid_file.to_str().unwrap().into(),
        ];
        let child = tokio::spawn(run(
            argv,
            options(None, 5_000),
            dir.path().to_path_buf(),
            receiver,
        ));
        let pid: i32 = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(&pid_file) {
                    if let Ok(pid) = pid.parse() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), child)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result.exit_code, None);
        assert!(result.stderr.contains("killed"));
        // SAFETY: signal 0 checks that the owned child has been reaped.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[tokio::test]
    async fn output_caps_drain_streams_and_report_truncation() {
        let result = command(
            &[
                "sh",
                "-c",
                "head -c 17000000 /dev/zero; head -c 17000000 /dev/zero >&2",
            ],
            options(None, 10_000),
        )
        .await;
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.len(), OUTPUT_LIMIT);
        assert!(result.stderr.len() <= OUTPUT_LIMIT);
        assert!(result.stderr.contains("output truncated"));
        let mut capture = Capture::default();
        capture.append(&vec![0xff; OUTPUT_LIMIT + 128]);
        let (text, truncated) = capture.text();
        assert!(truncated);
        assert_eq!(text.len(), OUTPUT_LIMIT - OUTPUT_LIMIT % 3);
        assert!(text.chars().all(|c| c == '\u{fffd}'));
    }

    #[tokio::test]
    async fn cancellation_is_scoped_and_reload_cancels_all_owners() {
        let (one, mut one_rx) = oneshot::channel();
        let (two, mut two_rx) = oneshot::channel();
        RUNNING.lock().insert(("one".into(), 1), one);
        RUNNING.lock().insert(("two".into(), 2), two);
        cancel_plugin("one");
        assert_eq!(one_rx.try_recv(), Err(oneshot::error::TryRecvError::Closed));
        assert_eq!(two_rx.try_recv(), Err(oneshot::error::TryRecvError::Empty));
        cancel_all();
        assert!(two_rx.await.is_err());
    }
}
