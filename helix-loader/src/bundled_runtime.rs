//! Portable runtime data for Cargo installs. User runtime paths remain higher
//! priority; this content-addressed cache contains no downloaded native parsers.
use std::{fs, io, path::Path};

include!(concat!(env!("OUT_DIR"), "/bundled_runtime.rs"));

const COMPLETE: &str = ".pure-runtime-complete";

pub(crate) fn install(destination: &Path) -> io::Result<()> {
    if destination.join(COMPLETE).is_file() {
        return Ok(());
    }
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "runtime cache has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".pure-runtime-")
        .tempdir_in(parent)?;
    for (relative, contents) in FILES {
        let path = staging.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)?;
    }
    fs::write(staging.path().join(COMPLETE), ID)?;
    // Publish only a complete tree. Two editor processes can initialize the
    // same cache concurrently without exposing partially written assets.
    match fs::rename(staging.path(), destination) {
        Ok(()) => Ok(()),
        Err(_) if destination.join(COMPLETE).is_file() => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cargo_install_has_queries_themes_and_a_tutor() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join(ID);
        install(&destination).unwrap();
        for name in ["queries/rust/highlights.scm", "themes/acid.toml", "tutor"] {
            assert!(destination.join(name).is_file(), "missing {name}");
        }
        assert!(!destination.join("grammars").exists());
        // Subsequent launches do not rewrite existing files.
        fs::write(destination.join("tutor"), "unchanged").unwrap();
        install(&destination).unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("tutor")).unwrap(),
            "unchanged"
        );
    }

    #[test]
    fn concurrent_launches_install_one_complete_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join(ID);
        std::thread::scope(|scope| {
            let a = scope.spawn(|| install(&destination));
            let b = scope.spawn(|| install(&destination));
            a.join().unwrap().unwrap();
            b.join().unwrap().unwrap();
        });
        assert_eq!(fs::read_to_string(destination.join(COMPLETE)).unwrap(), ID);
        for (relative, bytes) in FILES {
            assert_eq!(fs::read(destination.join(relative)).unwrap(), *bytes);
        }
    }
}
