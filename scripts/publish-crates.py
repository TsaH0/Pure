#!/usr/bin/env python3
"""Publish Pure's registry packages in dependency order, or resume a release.

Run check-crates.py first. Existing versions are skipped only if their registry
repository matches this project. Cargo uses its own credential provider; this
script never reads or prints registry tokens. With --dry-run nothing is uploaded.
Crates.io's explicit rate-limit retry timestamps are respected automatically.
"""
import argparse
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
import json
import math
from pathlib import Path
import re
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "https://github.com/TsaH0/Pure"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--dry-run", action="store_true")
parser.add_argument("--no-verify", action="store_true", help="Skip Cargo's repeated build after check-crates.py has passed")
parser.add_argument("--status-file", type=Path, help="Write progress as JSON (contains no credentials)")
parser.add_argument("--not-before", help="Honor a retry date from an earlier crates.io response")
options = parser.parse_args()
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=ROOT))
packages = {p["name"]: p for p in metadata["packages"] if p["name"].startswith("pure-editor")}
for package in packages.values():
    if not package.get("description") or not (package.get("license") or package.get("license_file")):
        raise SystemExit(f"{package['name']} is missing required registry metadata")
remaining = dict(packages)
ordered = []
while remaining:
    ready = [name for name, p in remaining.items() if not any(
        d["name"] in remaining and d["kind"] != "dev" for d in p["dependencies"])]
    if not ready:
        raise SystemExit("Registry packages have a non-dev dependency cycle")
    for name in sorted(ready):
        ordered.append(remaining.pop(name))
status = {"state": "publishing", "completed_crates": [], "total_crates": len(ordered)}

def progress(**changes):
    status.update(changes)
    status["updated_at"] = datetime.now(timezone.utc).isoformat()
    if options.status_file:
        options.status_file.parent.mkdir(parents=True, exist_ok=True)
        temporary = options.status_file.with_suffix(".tmp")
        temporary.write_text(json.dumps(status, indent=2) + "\n")
        temporary.replace(options.status_file)

def wait_until(date):
    deadline = parsedate_to_datetime(date).timestamp() + 3
    seconds = max(0, math.ceil(deadline - time.time()))
    progress(state="rate_limited", next_retry=date)
    print(f"Respecting crates.io retry date {date}; waiting {seconds}s", flush=True)
    time.sleep(seconds)
    progress(state="publishing", next_retry=None)

progress()
if options.not_before:
    wait_until(options.not_before)
for package in ordered:
    name, version = package["name"], package["version"]
    progress(current_crate=name, version=version)
    request = urllib.request.Request(f"https://crates.io/api/v1/crates/{name}", headers={
        "User-Agent": "Pure release tooling (https://github.com/TsaH0/Pure)"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            remote = json.load(response)
        if remote["crate"].get("repository", "").rstrip("/") != REPOSITORY:
            progress(state="failed", error=f"{name} belongs to another project")
            raise SystemExit(status["error"])
        if any(v["num"] == version and not v["yanked"] for v in remote["versions"]):
            print(f"Already published: {name} {version}", flush=True)
            status["completed_crates"].append(name)
            progress()
            continue
    except urllib.error.HTTPError as error:
        if error.code != 404:
            progress(state="failed", error=f"Registry lookup: HTTP {error.code}")
            raise
    command = ["cargo", "publish", "-p", name, "--locked"]
    if options.dry_run:
        command.append("--dry-run")
    if options.no_verify:
        command.append("--no-verify")
    while True:
        print(f"Publishing {name} {version}", flush=True)
        process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True)
        lines = []
        for line in process.stdout:
            print(line, end="", flush=True)
            lines.append(line)
        if process.wait() == 0:
            if not options.dry_run:
                status["completed_crates"].append(name)
            progress()
            break
        output = "".join(lines)
        retry = re.search(r"try again after (.*? GMT)", output)
        if "429 Too Many Requests" in output and retry:
            wait_until(retry.group(1))
        else:
            progress(state="failed", error=f"cargo publish failed for {name}; see release log")
            raise SystemExit(process.returncode)
progress(state="dry_run_complete" if options.dry_run else "published", current_crate=None, next_retry=None)
