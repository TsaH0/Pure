#!/usr/bin/env python3
"""Verify a real crates.io install without replacing the user's editor."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--root", required=True, type=Path, help="Isolated Cargo install prefix")
parser.add_argument("--target-dir", type=Path)
parser.add_argument("--status-file", type=Path)
options = parser.parse_args()
source = Path(__file__).resolve().parents[1]
version = tomllib.loads((source / "Cargo.toml").read_text())["workspace"]["package"]["version"]

def progress(state, **details):
    status = json.loads(options.status_file.read_text()) if options.status_file and options.status_file.exists() else {}
    status.update(state=state, updated_at=datetime.now(timezone.utc).isoformat(), **details)
    if options.status_file:
        options.status_file.parent.mkdir(parents=True, exist_ok=True)
        temporary = options.status_file.with_suffix(".tmp")
        temporary.write_text(json.dumps(status, indent=2) + "\n")
        temporary.replace(options.status_file)

try:
    progress("verifying_install")
    with tempfile.TemporaryDirectory(prefix="pure-registry-install-check-") as temp:
        temp = Path(temp)
        command = ["cargo", "+1.90.0", "install", "pure-editor", "--version", version,
                   "--locked", "--root", str(options.root.resolve()), "--jobs", "4"]
        if options.target_dir:
            command += ["--target-dir", str(options.target_dir.resolve())]
        # Outside the workspace: no path patches or repository Cargo config.
        subprocess.run(command, cwd=temp, check=True)
        binary = options.root.resolve() / "bin" / ("pure.exe" if os.name == "nt" else "pure")
        reported = subprocess.check_output([str(binary), "--version"], cwd=temp, text=True)
        assert version in reported, reported
        environment = os.environ.copy()
        for key in ["HELIX_RUNTIME", "HELIX_DEFAULT_RUNTIME", "CARGO_MANIFEST_DIR"]:
            environment.pop(key, None)
        environment.update(XDG_CONFIG_HOME=str(temp / "config"), XDG_CACHE_HOME=str(temp / "cache"),
                           XDG_DATA_HOME=str(temp / "data"))
        health = subprocess.check_output([str(binary), "--health", "rust"], cwd=temp, env=environment, text=True)
        print(reported, health, flush=True)
        markers = list((temp / "cache" / "helix" / "runtime").glob("*/.pure-runtime-complete"))
        assert len(markers) == 1, "bundled runtime was not installed"
        for file in ["queries/rust/highlights.scm", "themes/acid.toml", "tutor"]:
            assert (markers[0].parent / file).is_file(), file
        assert not (markers[0].parent / "grammars").exists(), "native parsers should be opt-in"
    progress("verified", verified_binary=str(binary), install_command="cargo install pure-editor --locked")
    print("Registry install and clean-profile bundled runtime verified.", flush=True)
except Exception as error:
    progress("verification_failed", error=str(error))
    raise
