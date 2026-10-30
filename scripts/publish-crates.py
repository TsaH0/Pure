#!/usr/bin/env python3
"""Publish Pure's registry packages in dependency order, or resume a release.

Run check-crates.py first. Existing versions are skipped only if their registry
repository matches this project. Cargo uses its own credential provider; this
script never reads or prints registry tokens. With --dry-run nothing is uploaded.
"""
import argparse
import json
from pathlib import Path
import subprocess
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "https://github.com/TsaH0/Pure"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--dry-run", action="store_true")
parser.add_argument("--no-verify", action="store_true", help="Skip Cargo's repeated build after check-crates.py has passed")
options = parser.parse_args()
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=ROOT))
packages = {p["name"]: p for p in metadata["packages"] if p["name"].startswith("pure-editor")}
remaining = dict(packages)
ordered = []
while remaining:
    ready = [name for name, p in remaining.items() if not any(
        d["name"] in remaining and d["kind"] != "dev" for d in p["dependencies"])]
    if not ready:
        raise SystemExit("Registry packages have a non-dev dependency cycle")
    for name in sorted(ready):
        ordered.append(remaining.pop(name))
for package in ordered:
    name, version = package["name"], package["version"]
    request = urllib.request.Request(f"https://crates.io/api/v1/crates/{name}", headers={
        "User-Agent": "Pure release tooling (https://github.com/TsaH0/Pure)"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            remote = json.load(response)
        if remote["crate"].get("repository", "").rstrip("/") != REPOSITORY:
            raise SystemExit(f"{name} is owned by another project; stopping")
        if any(v["num"] == version and not v["yanked"] for v in remote["versions"]):
            print(f"Already published: {name} {version}", flush=True)
            continue
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
    print(f"Publishing {name} {version}", flush=True)
    command = ["cargo", "publish", "-p", name, "--locked"]
    if options.dry_run:
        command.append("--dry-run")
    if options.no_verify:
        command.append("--no-verify")
    subprocess.run(command, cwd=ROOT, check=True)
