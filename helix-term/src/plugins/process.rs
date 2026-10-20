//! Programs started by plugins: `spawn` (in the background, with captured
//! output) and `run-interactive` (with the terminal handed over).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use helix_event::runtime_local;
use helix_plugin::{Callback, ProcessOutput};
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use crate::interactive::Interactive;

/// Most output kept per stream; the rest is read and dropped.
const MAX_OUTPUT: usize = 16 << 20;
/// After a program exits, how long to wait for the rest of its output. A
/// child it started may keep the pipes open.
const OUTPUT_GRACE: Duration = Duration::from_millis(500);

runtime_local! {
    /// Running programs by `(plugin, request id)`. Dropping the sender kills
    /// the program.
    static RUNNING: Mutex<BTreeMap<(String, u32), oneshot::Sender<()>>> = Mutex::new(BTreeMap::new());
}

/// Run `argv` in the background and deliver its output to the plugin.
pub fn spawn(
    plugin: String,
    id: u32,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    stdin: Option<String>,
    timeout: Duration,
    stream: bool,
) {
    let (kill_tx, kill_rx) = oneshot::channel();
    RUNNING.lock().insert((plugin.clone(), id), kill_tx);
    tokio::spawn(async move {
        let (output, forward) = stream
            .then(|| {
                let (sender, mut receiver) = mpsc::channel::<String>(1);
                let plugin = plugin.clone();
                let task = tokio::spawn(async move {
                    while let Some(chunk) = receiver.recv().await {
                        super::deliver_async(plugin.clone(), Callback::ProcessOutput { id, chunk })
                            .await;
                    }
                });
                (sender, task)
            })
            .unzip();
        let result = run(argv, cwd, stdin, timeout, kill_rx, output).await;
        if let Some(forward) = forward {
            let _ = forward.await;
        }
        RUNNING.lock().remove(&(plugin.clone(), id));
        super::deliver_async(plugin, Callback::ProcessDone { id, result }).await;
    });
}

/// Kill a program started with [`spawn`]. Its result is still delivered.
pub fn kill(plugin: &str, id: u32) {
    // Dropping the sender is enough.
    RUNNING.lock().remove(&(plugin.to_string(), id));
}

/// Kill every plugin's programs (when plugins are reloaded).
pub fn kill_all() {
    RUNNING.lock().clear();
}

/// Stop work belonging to a plugin disabled by a trap or timeout.
pub fn kill_plugin(plugin: &str) {
    RUNNING.lock().retain(|(owner, _), _| owner != plugin);
}

async fn run(
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    stdin: Option<String>,
    timeout: Duration,
    kill: oneshot::Receiver<()>,
    output: Option<mpsc::Sender<String>>,
) -> Result<ProcessOutput, String> {
    let program = &argv[0];
    let mut command = Command::new(program);
    command
        .args(&argv[1..])
        .current_dir(cwd.unwrap_or_else(helix_stdx::env::current_working_dir))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Its own process group, so programs it starts are killed with it.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to run '{program}': {err}"))?;

    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Closed when written (or when the program stops reading).
        tokio::spawn(async move {
            let _ = pipe.write_all(input.as_bytes()).await;
        });
    }
    let stdout = Capture::start(child.stdout.take(), output);
    let stderr = Capture::start(child.stderr.take(), None);

    let mut note = None;
    let status = tokio::select! {
        status = child.wait() => status.ok(),
        _ = tokio::time::sleep(timeout) => {
            note = Some(format!("[killed after {} ms]", timeout.as_millis()));
            kill_group(&mut child).await;
            None
        }
        _ = kill => {
            note = Some("[killed]".to_string());
            kill_group(&mut child).await;
            None
        }
    };

    let stdout = stdout.finish().await;
    let mut stderr = stderr.finish().await;
    for (truncated, name) in [(stdout.1, "stdout"), (stderr.1, "stderr")] {
        if truncated {
            stderr
                .0
                .push_str(&format!("\n[{name} truncated at 16 MiB]"));
        }
    }
    if let Some(note) = note {
        if !stderr.0.is_empty() && !stderr.0.ends_with('\n') {
            stderr.0.push('\n');
        }
        stderr.0.push_str(&note);
    }
    Ok(ProcessOutput {
        exit_code: status.and_then(|status| status.code()),
        stdout: stdout.0,
        stderr: stderr.0,
    })
}

/// Kill the program and everything it started (its process group).
async fn kill_group(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: plain system call; the group was created at spawn.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
}

/// Output of one pipe, read in the background. Stream mode retains only
/// the unfinished line; the cap counts all bytes, including delivered ones.
struct Capture {
    buffer: Arc<Mutex<Vec<u8>>>,
    truncated: Arc<AtomicBool>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Capture {
    fn start(
        reader: Option<impl AsyncRead + Unpin + Send + 'static>,
        output: Option<mpsc::Sender<String>>,
    ) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let truncated = Arc::new(AtomicBool::new(false));
        let (stop, mut stopped) = oneshot::channel();
        let task = reader.map(|mut reader| {
            let (buffer, truncated) = (buffer.clone(), truncated.clone());
            tokio::spawn(async move {
                let mut chunk = [0; 8192];
                let mut accepted = 0;
                let mut deadline = None;
                loop {
                    // Grace limits pipe reads after exit, not delivery to a
                    // slow consumer. Backpressure must not discard batches.
                    let read = if let Some(deadline) = deadline {
                        match tokio::time::timeout_at(deadline, reader.read(&mut chunk)).await {
                            Ok(read) => read,
                            Err(_) => break,
                        }
                    } else {
                        tokio::select! {
                            _ = &mut stopped => {
                                deadline = Some(tokio::time::Instant::now() + OUTPUT_GRACE);
                                continue;
                            }
                            read = reader.read(&mut chunk) => read,
                        }
                    };
                    let Ok(n @ 1..) = read else { break };
                    let room = MAX_OUTPUT - accepted;
                    if n > room {
                        truncated.store(true, Ordering::Relaxed);
                    }
                    let bytes = &chunk[..n.min(room)];
                    accepted += bytes.len();
                    let lines = {
                        let mut buffer = buffer.lock();
                        let old_len = buffer.len();
                        buffer.extend_from_slice(bytes);
                        // Search only the new bytes: rescanning an unfinished
                        // long line on every read would be quadratic.
                        output.as_ref().and_then(|_| {
                            let end = old_len + bytes.iter().rposition(|byte| *byte == b'\n')? + 1;
                            let rest = buffer.split_off(end);
                            Some(std::mem::replace(&mut *buffer, rest))
                        })
                    };
                    if let (Some(lines), Some(output)) = (lines, &output) {
                        // One queued batch here; the editor job queue is also
                        // bounded. No per-line tasks or flush timer.
                        if output.send(decode(lines)).await.is_err() {
                            break;
                        }
                    }
                }
            })
        });
        Self {
            buffer,
            truncated,
            stop: Some(stop),
            task,
        }
    }

    /// Signal the post-exit read deadline, then drain already-read batches.
    async fn finish(mut self) -> (String, bool) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        let bytes = std::mem::take(&mut *self.buffer.lock());
        (decode(bytes), self.truncated.load(Ordering::Relaxed))
    }
}

fn decode(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// Hand the terminal to `argv`, then deliver its exit code and, with
/// `chooser_file`, what it wrote to the file substituted for `{chooser}`.
pub fn interactive(
    plugin: String,
    id: u32,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    chooser_file: bool,
    wait_for_key: bool,
) -> Result<Interactive, String> {
    let chooser = if chooser_file {
        Some(
            create_chooser_file()
                .map_err(|err| format!("failed to create a chooser file: {err}"))?,
        )
    } else {
        None
    };
    let args = argv.iter().map(|arg| match &chooser {
        Some(path) => arg.replace("{chooser}", &path.to_string_lossy()),
        None => arg.clone(),
    });
    let args: Vec<String> = args.collect();
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .current_dir(cwd.unwrap_or_else(helix_stdx::env::current_working_dir));
    let owner = plugin.clone();
    let cancelled = plugin.clone();
    let mut interactive = Interactive::new(command, move |_editor, _, status| {
        let result = status
            .map_err(|err| format!("failed to run '{}': {err}", args[0]))
            .and_then(|status| {
                let (stdout, truncated) = chooser
                    .as_ref()
                    .map(|path| read_chooser(path))
                    .transpose()
                    .map_err(|err| format!("failed to read chooser: {err}"))?
                    .unwrap_or_default();
                Ok(ProcessOutput {
                    exit_code: status.code(),
                    stdout,
                    stderr: if truncated {
                        "chooser output truncated at 16 MiB".into()
                    } else {
                        String::new()
                    },
                })
            });
        super::deliver_later(plugin, Callback::ProcessDone { id, result });
    });
    // Replaced by another program before it ran (or the editor quit): the
    // plugin still gets its answer, so the request doesn't stay open.
    interactive.on_cancel = crate::interactive::OnCancel::new(move || {
        if tokio::runtime::Handle::try_current().is_ok() {
            super::deliver_later(
                cancelled,
                Callback::ProcessDone {
                    id,
                    result: Err("cancelled before it could run (another program took the \
                                 terminal, or the editor quit)"
                        .into()),
                },
            );
        }
    });
    interactive.before_run = Some(Box::new(move || {
        super::with_host(|host| host.has_request(&owner, id, helix_plugin::PendingKind::Process))
            .unwrap_or(false)
    }));
    if wait_for_key {
        // Like `!`: the output stays on a cleared screen until a key.
        interactive.clear_screen = true;
        interactive.wait_for_key = true;
    }
    Ok(interactive)
}

/// A chooser is untrusted process output: its path may have been replaced.
/// Open without waiting for a FIFO writer, then check the opened descriptor
/// (rather than racing a path metadata check against the open).
fn read_chooser(path: &Path) -> std::io::Result<(String, bool)> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        // Some choosers delete the empty file when cancelled.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok((String::new(), false))
        }
        Err(err) => return Err(err),
    };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("chooser must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_OUTPUT + 1) as u64).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > MAX_OUTPUT;
    bytes.truncate(MAX_OUTPUT);
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

/// A new, empty file only the user can read, for a program to write its
/// choice to.
#[derive(Debug, PartialEq, Eq)]
struct ChooserFile(PathBuf);

impl std::ops::Deref for ChooserFile {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for ChooserFile {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for ChooserFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn create_chooser_file() -> std::io::Result<ChooserFile> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    loop {
        let path = std::env::temp_dir().join(format!(
            "pure-chooser-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match options.open(&path) {
            Ok(_) => return Ok(ChooserFile(path)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["sh".into(), "-c".into(), script.into()]
    }

    async fn run_sh(script: &str, stdin: Option<&str>, timeout: Duration) -> ProcessOutput {
        let (_kill, kill_rx) = oneshot::channel();
        run(
            sh(script),
            None,
            stdin.map(str::to_owned),
            timeout,
            kill_rx,
            None,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn output_and_exit_code() {
        let out = run_sh(
            "echo hi; echo err >&2; exit 3",
            None,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(out.exit_code, Some(3));
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.stderr, "err\n");
    }

    #[tokio::test]
    async fn streaming_delivers_lines_before_exit_and_keeps_utf8_and_tail() {
        let (_kill, kill_rx) = oneshot::channel();
        let (tx, mut rx) = mpsc::channel(1);
        let task = tokio::spawn(run(
            sh("printf 'first\\n'; sleep 1; printf '\\303'; sleep 0.02; printf '\\251\\nlast'; printf err >&2"),
            None, None, Duration::from_secs(5), kill_rx, Some(tx),
        ));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), rx.recv())
                .await
                .unwrap()
                .unwrap(),
            "first\n"
        );
        assert!(
            !task.is_finished(),
            "the first line arrives while the child runs"
        );
        assert_eq!(rx.recv().await.unwrap(), "é\n");
        assert!(rx.recv().await.is_none());
        let out = task.await.unwrap().unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(out.stdout, "last");
        assert_eq!(out.stderr, "err");
    }

    #[tokio::test]
    async fn streamed_bytes_count_toward_the_cumulative_output_cap() {
        let (mut writer, reader) = tokio::io::duplex(8192);
        let (tx, mut rx) = mpsc::channel::<String>(1);
        let capture = Capture::start(Some(reader), Some(tx));
        let collect = tokio::spawn(async move {
            let mut count = 0;
            while let Some(chunk) = rx.recv().await {
                assert!(chunk.ends_with('\n'));
                count += chunk.len();
            }
            count
        });
        let bytes = b"x\n".repeat(MAX_OUTPUT / 2 + 1);
        writer.write_all(&bytes).await.unwrap();
        drop(writer);
        let (tail, truncated) = capture.finish().await;
        assert!(truncated);
        assert!(tail.is_empty());
        assert_eq!(collect.await.unwrap(), MAX_OUTPUT);
    }

    #[tokio::test]
    async fn backpressure_does_not_discard_batches_when_the_pipe_grace_expires() {
        let (mut writer, reader) = tokio::io::duplex(32);
        let (tx, mut rx) = mpsc::channel(1);
        let capture = Capture::start(Some(reader), Some(tx));
        writer.write_all(b"first\n").await.unwrap();
        // Wait for the first batch to fill the channel.
        tokio::time::timeout(Duration::from_secs(2), async {
            while rx.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first batch queued");
        writer.write_all(b"second\ntail").await.unwrap();
        drop(writer);
        let finish = tokio::spawn(capture.finish());
        tokio::time::sleep(OUTPUT_GRACE + Duration::from_millis(100)).await;
        assert_eq!(rx.recv().await.unwrap(), "first\n");
        assert_eq!(rx.recv().await.unwrap(), "second\n");
        assert!(rx.recv().await.is_none());
        assert_eq!(finish.await.unwrap(), ("tail".into(), false));
    }

    #[tokio::test]
    async fn disabling_one_plugin_kills_only_its_processes() {
        let (first, mut first_rx) = oneshot::channel();
        let (second, mut second_rx) = oneshot::channel();
        RUNNING.lock().insert(("disable-first".into(), 1), first);
        RUNNING.lock().insert(("disable-second".into(), 2), second);
        kill_plugin("disable-first");
        assert_eq!(
            first_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        );
        assert_eq!(
            second_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        kill_plugin("disable-second");
    }

    #[tokio::test]
    async fn stdin_cwd_and_lossy_utf8() {
        let out = run_sh(
            "cat; printf '\\377'",
            Some("in put"),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(out.stdout, "in put\u{FFFD}");
        let dir = tempfile::tempdir().unwrap();
        let (_kill, kill_rx) = oneshot::channel();
        let out = run(
            sh("pwd"),
            Some(dir.path().to_path_buf()),
            None,
            Duration::from_secs(5),
            kill_rx,
            None,
        )
        .await
        .unwrap();
        let expected = dir.path().canonicalize().unwrap();
        assert_eq!(
            PathBuf::from(out.stdout.trim()).canonicalize().unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn timeout_and_kill() {
        let started = std::time::Instant::now();
        // The background child keeps the pipes open after `sh` is killed.
        let out = run_sh("sleep 30 & sleep 30", None, Duration::from_millis(200)).await;
        assert_eq!(out.exit_code, None);
        assert!(out.stderr.contains("killed after 200 ms"), "{}", out.stderr);
        assert!(started.elapsed() < Duration::from_secs(5));

        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(run(
            sh("sleep 30"),
            None,
            None,
            Duration::from_secs(30),
            kill_rx,
            None,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(kill_tx);
        let out = task.await.unwrap().unwrap();
        assert_eq!(out.exit_code, None);
        assert!(out.stderr.ends_with("[killed]"), "{}", out.stderr);
    }

    #[tokio::test]
    async fn missing_program() {
        let (_kill, kill_rx) = oneshot::channel();
        let err = run(
            vec!["/nonexistent/program".into()],
            None,
            None,
            Duration::from_secs(1),
            kill_rx,
            None,
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("failed to run '/nonexistent/program'"),
            "{err}"
        );
    }

    #[test]
    fn chooser_files_are_new_and_private() {
        let a = create_chooser_file().unwrap();
        let b = create_chooser_file().unwrap();
        assert_ne!(a, b);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let a_path = a.0.clone();
        let b_path = b.0.clone();
        drop((a, b));
        assert!(!a_path.exists());
        assert!(!b_path.exists());
    }

    #[test]
    fn chooser_reads_are_capped_and_report_only_actual_truncation() {
        let file = create_chooser_file().unwrap();
        let writer = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        writer.set_len(MAX_OUTPUT as u64).unwrap();
        let (output, truncated) = read_chooser(&file).unwrap();
        assert_eq!(output.len(), MAX_OUTPUT);
        assert!(!truncated, "an exactly full output is not truncated");
        writer.set_len((MAX_OUTPUT + 1) as u64).unwrap();
        let (output, truncated) = read_chooser(&file).unwrap();
        assert_eq!(output.len(), MAX_OUTPUT);
        assert!(truncated);
        std::fs::write(&file, b"selected\xff").unwrap();
        assert_eq!(
            read_chooser(&file).unwrap(),
            ("selected\u{fffd}".into(), false)
        );
        std::fs::remove_file(&file).unwrap();
        assert_eq!(read_chooser(&file).unwrap(), (String::new(), false));
    }

    #[cfg(unix)]
    #[test]
    fn a_chooser_replaced_with_a_fifo_is_refused_without_waiting_for_a_writer() {
        use std::os::unix::fs::OpenOptionsExt;
        let chooser = create_chooser_file().unwrap();
        std::fs::remove_file(&chooser).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(&chooser.0)
            .status()
            .unwrap()
            .success());
        let path = chooser.0.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || tx.send(read_chooser(&path)).unwrap());
        let result = rx.recv_timeout(Duration::from_secs(2));
        if result.is_err() {
            // Unblock a regressed blocking open, so a failure does not leave
            // an immortal test worker behind.
            drop(
                std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&chooser.0),
            );
        }
        thread.join().unwrap();
        let error = result
            .expect("chooser open waited for a FIFO writer")
            .unwrap_err();
        assert!(error.to_string().contains("regular file"), "{error}");
    }

    #[test]
    fn dropping_a_queued_interactive_request_removes_its_chooser() {
        let request = interactive(
            "test".into(),
            1,
            vec!["true".into(), "{chooser}".into()],
            None,
            true,
            false,
        )
        .unwrap();
        let path = PathBuf::from(request.command.as_std().get_args().next().unwrap());
        assert!(path.is_file());
        assert!(
            !request.can_run(),
            "an absent owner cannot hand over the terminal"
        );
        drop(request);
        assert!(
            !path.exists(),
            "a request discarded before launch must clean up"
        );
    }
}
