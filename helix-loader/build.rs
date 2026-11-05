use std::borrow::Cow;
use std::path::Path;
use std::process::Command;

const MAJOR: &str = env!("CARGO_PKG_VERSION_MAJOR");
const MINOR: &str = env!("CARGO_PKG_VERSION_MINOR");
const PATCH: &str = env!("CARGO_PKG_VERSION_PATCH");

fn main() {
    bundle_runtime();
    let git_hash = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|x| String::from_utf8(x.stdout).ok())
        .or_else(|| option_env!("HELIX_NIX_BUILD_REV").map(|s| s.to_string()));

    let minor = if MINOR.len() == 1 {
        // Print single-digit months in '0M' format
        format!("0{MINOR}")
    } else {
        MINOR.to_string()
    };
    let calver = if MAJOR == "0" {
        env!("CARGO_PKG_VERSION").to_string()
    } else if PATCH == "0" {
        format!("{MAJOR}.{minor}")
    } else {
        format!("{MAJOR}.{minor}.{PATCH}")
    };
    let version: Cow<_> = match &git_hash {
        Some(git_hash) => format!("{} ({})", calver, &git_hash[..8]).into(),
        None => calver.into(),
    };

    println!(
        "cargo:rustc-env=BUILD_TARGET={}",
        std::env::var("TARGET").unwrap()
    );

    println!("cargo:rustc-env=VERSION_AND_GIT_HASH={}", version);

    if git_hash.is_none() {
        return;
    }

    // we need to revparse because the git dir could be anywhere if you are
    // using detached worktrees but there is no good way to obtain an OsString
    // from command output so for now we can't accept non-utf8 paths here
    // probably rare enough where it doesn't matter tough we could use gitoxide
    // here but that would be make it a hard dependency and slow compile times
    let Some(git_dir): Option<String> = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|x| String::from_utf8(x.stdout).ok())
        .map(|x| x.trim().to_string())
    else {
        return;
    };
    // If heads starts pointing at something else (different branch)
    // we need to return
    let head = Path::new(&git_dir).join("HEAD");
    if head.exists() {
        println!("cargo:rerun-if-changed={}", head.display());
    }
    // if the thing head points to (branch) itself changes
    // we need to return
    let Some(head_ref): Option<String> = Command::new("git")
        .args(["symbolic-ref", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|x| String::from_utf8(x.stdout).ok())
        .map(|x| x.trim().to_string())
    else {
        return;
    };
    let head_ref = Path::new(&git_dir).join(head_ref);
    if head_ref.exists() {
        println!("cargo:rerun-if-changed={}", head_ref.display());
    }
}

// Embed only portable runtime data, never local parser sources or binaries.
fn bundle_runtime() {
    use sha2::{Digest, Sha256};
    use std::{env, fs, path::PathBuf};
    fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
        for entry in fs::read_dir(dir).expect("read bundled runtime directory") {
            let path = entry.expect("read bundled runtime entry").path();
            if path.is_dir() {
                collect(root, &path, files);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("runtime file under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((relative, path));
            }
        }
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("runtime");
    let mut files = Vec::new();
    collect(&root, &root.join("queries"), &mut files);
    collect(&root, &root.join("themes"), &mut files);
    files.push(("tutor".into(), root.join("tutor")));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hash = Sha256::new();
    let mut code = String::from("pub const FILES: &[(&str, &[u8])] = &[\n");
    for (relative, path) in files {
        println!("cargo:rerun-if-changed={}", path.display());
        hash.update(relative.as_bytes());
        hash.update(fs::read(&path).expect("read bundled runtime asset"));
        code.push_str(&format!("({relative:?}, include_bytes!({path:?})),\n"));
    }
    code.push_str("];\n");
    let id: String = hash.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    code.push_str(&format!("pub const ID: &str = {id:?};\n"));
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bundled_runtime.rs"),
        code,
    )
    .expect("write bundled runtime index");
}
