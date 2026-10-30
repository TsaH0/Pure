#!/usr/bin/env python3
"""Check the actual publish archives, with no unpublished registry names needed.

Internal dependencies are patched to the extracted archives, never the checkout.
This catches missing WIT, C sources, themes, language config and runtime assets.
Requires Python 3.12+ for safe tar extraction.
"""
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=ROOT))
packages = [p for p in metadata["packages"] if p["name"].startswith("pure-editor")]
target = Path(metadata["target_directory"])

def patches(path, directories):
    path.write_text("[patch.crates-io]\n" + "".join(
        f"{json.dumps(name)} = {{ path = {json.dumps(str(directory))} }}\n"
        for name, directory in directories.items()))

with tempfile.TemporaryDirectory(prefix="pure-crates-check-") as temp:
    temp = Path(temp)
    config = temp / "patches.toml"
    patches(config, {p["name"]: Path(p["manifest_path"]).parent for p in packages})
    extracted = {}
    for package in packages:
        name, version = package["name"], package["version"]
        command = ["cargo", "package", "-p", name, "--allow-dirty", "--no-verify", "--config", str(config)]
        result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
        if result.returncode:
            raise SystemExit(result.stdout + result.stderr)
        archive = target / "package" / f"{name}-{version}.crate"
        with tarfile.open(archive) as tar:
            tar.extractall(temp, filter="data")
        extracted[name] = temp / f"{name}-{version}"
        print(f"Packaged {name}: {archive.stat().st_size:,} bytes", flush=True)
    patches(config, extracted)
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target)
    environment["HELIX_DISABLE_AUTO_GRAMMAR_BUILD"] = "1"
    subprocess.run(["cargo", "check", "--manifest-path", str(extracted["pure-editor"] / "Cargo.toml"),
                    "--config", str(config)], env=environment, cwd=temp, check=True)
    print(f"Verified {len(packages)} extracted crates without checkout dependencies.")
