use super::*;

use std::time::Duration;

use helix_term::application::Application;
use serde_json::{json, Value};

/// Send `request` to the editor's socket while its event loop runs.
async fn ask(
    app: &mut Application,
    socket: &std::path::Path,
    request: Value,
) -> anyhow::Result<Value> {
    let socket = socket.to_path_buf();
    let reply = tokio::task::spawn_blocking(move || helix_term::remote::request(&socket, request));
    for _ in 0..200 {
        if reply.is_finished() {
            break;
        }
        app.editor.reset_idle_timer();
        helpers::run_event_loop_until_idle(app).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    reply.await?
}

#[tokio::test(flavor = "multi_thread")]
async fn other_programs_drive_the_editor() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[o|]#ne\n")
        .build()?;
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("pure.sock");
    helix_term::remote::listen(socket.clone())?;
    // Only this user may connect.
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&socket)?.permissions().mode() & 0o777,
        0o600
    );

    let state = ask(&mut app, &socket, json!({ "method": "state" })).await?;
    assert_eq!(state["mode"], "normal");
    assert_eq!(state["line"], 1);

    ask(
        &mut app,
        &socket,
        json!({ "method": "keys", "params": { "keys": "ihello <esc>" } }),
    )
    .await?;
    assert_eq!(
        helix_view::doc!(app.editor).text().to_string(),
        "hello one\n"
    );

    let file = dir.path().join("written.txt");
    let line = format!(":write {}", file.display());
    ask(
        &mut app,
        &socket,
        json!({ "method": "command", "params": { "line": line } }),
    )
    .await?;
    for _ in 0..100 {
        if std::fs::read_to_string(&file).is_ok_and(|text| text == "hello one\n") {
            break;
        }
        helpers::run_event_loop_until_idle(&mut app).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(std::fs::read_to_string(&file)?, "hello one\n");

    let other = dir.path().join("other.txt");
    std::fs::write(&other, "other\n")?;
    let paths = json!([other]);
    ask(
        &mut app,
        &socket,
        json!({ "method": "open", "params": { "paths": paths } }),
    )
    .await?;
    let state = ask(&mut app, &socket, json!({ "method": "state" })).await?;
    assert_eq!(state["file"], json!(helix_stdx::path::canonicalize(&other)));

    // The command line client: `pure --server=... --remote-...`.
    let pure = |args: Vec<String>| {
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_pure"))
                .args(args)
                .env_remove("PURE_LISTEN")
                .output()
                .unwrap()
        })
    };
    let server = format!("--server={}", socket.display());
    let run = pure(vec![
        server.clone(),
        "--remote-send".into(),
        "ggiclient <esc>".into(),
    ]);
    while !run.is_finished() {
        helpers::run_event_loop_until_idle(&mut app).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(run.await?.status.success());
    assert_eq!(
        helix_view::doc!(app.editor).text().to_string(),
        "client other\n"
    );
    let run = pure(vec![server.clone(), "--remote-state".into()]);
    while !run.is_finished() {
        helpers::run_event_loop_until_idle(&mut app).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let output = run.await?;
    let printed: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(printed["modified"], true);
    // No server: an error, not an editor started.
    let output = pure(vec!["--remote-state".into()]).await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("PURE_LISTEN"));

    let err = ask(&mut app, &socket, json!({ "method": "nope" }))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown method"), "{err}");
    let err = ask(
        &mut app,
        &socket,
        json!({ "method": "command", "params": { "line": "no-such-command" } }),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no-such-command"), "{err}");
    helix_term::remote::stop();
    assert!(!socket.exists());
    Ok(())
}
