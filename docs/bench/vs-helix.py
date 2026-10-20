"""Drive editor binaries through a pty with identical input; compare
startup time, CPU time, peak memory and idle activity."""
import glob, json, os, pty, re, select, statistics, sys, threading, time

SP = os.path.dirname(os.path.abspath(__file__))
FILE = os.path.join(SP, "work", "big.rs")
CFG = os.path.join(os.path.dirname(SP), "bench-cfg")
ESC = b"\x1b"


def spawn(binary, runtime, data):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(os.path.dirname(FILE))
        env = {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color", "HOME": data,
               "XDG_CONFIG_HOME": CFG, "XDG_DATA_HOME": data, "XDG_CACHE_HOME": data,
               "HELIX_RUNTIME": runtime}
        os.execve(binary, [binary, FILE], env)
    import fcntl, struct, termios
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    out = bytearray()
    stop = threading.Event()

    def reader():
        while not stop.is_set():
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                out.extend(chunk)

    threading.Thread(target=reader, daemon=True).start()
    return pid, fd, out, stop


def wait_for(out, needle, timeout=20):
    start = time.monotonic()
    while needle not in out:
        if time.monotonic() - start > timeout:
            raise TimeoutError(needle)
        time.sleep(0.0005)


def send(fd, key):
    os.write(fd, key)
    # An escape followed quickly by a key reads as Alt+key.
    time.sleep(0.06 if key == ESC else 0.003)


def finish(pid, fd, stop, out):
    send(fd, ESC)
    send(fd, ESC)
    os.write(fd, b":q!\r")
    deadline = time.monotonic() + 30
    while True:
        done, _, usage = os.wait4(pid, os.WNOHANG)
        if done:
            break
        if time.monotonic() > deadline:
            os.kill(pid, 9)
            screen = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b" ", bytes(out[-4000:]))
            sys.stderr.write(screen.decode("utf8", "replace")[-600:] + "\n")
            raise TimeoutError("editor did not quit")
        time.sleep(0.01)
    stop.set()
    os.close(fd)
    return usage


def proc_stats(pid):
    switches = 0
    for status in glob.glob(f"/proc/{pid}/task/*/status"):
        try:
            for line in open(status):
                if "ctxt_switches" in line:
                    switches += int(line.split()[1])
        except OSError:
            pass
    stat = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    return switches, (int(stat[11]) + int(stat[12])) / os.sysconf("SC_CLK_TCK")


WORK = (
    [b"j"] * 400
    + [b"/", b"fn ", b"\r"] + [b"n"] * 60
    + [b"g", b"g", b"o"] + [bytes([c]) for c in b"let value = compute(first, second); "] * 6 + [ESC]
    + [b"w"] * 200 + [b"g", b"e", b"g", b"g"]
    + [b"x"] * 40 + [b"s", b"self\r", b";", b","]
    + [b"u"] * 3 + [b"\x06", b"\x02"] * 30
)


def run(binary, runtime, mode):
    data = os.path.join(SP, "data", os.path.basename(binary))
    os.makedirs(data, exist_ok=True)
    start = time.monotonic()
    pid, fd, out, stop = spawn(binary, runtime, data)
    wait_for(out, b"big.rs")
    result = {"startup_ms": (time.monotonic() - start) * 1000}
    if mode == "idle":
        time.sleep(3)
        s0, c0 = proc_stats(pid)
        time.sleep(5)
        s1, c1 = proc_stats(pid)
        result["idle_wakeups_5s"] = s1 - s0
        result["idle_cpu_ms_5s"] = (c1 - c0) * 1000
    elif mode == "work":
        time.sleep(0.5)
        for key in WORK:
            send(fd, key)
        time.sleep(0.5)
    usage = finish(pid, fd, stop, out)
    result["cpu_ms"] = (usage.ru_utime + usage.ru_stime) * 1000
    result["max_rss_mib"] = usage.ru_maxrss / 1024
    return result


editors = json.loads(sys.argv[1])
mode, rounds = sys.argv[2], int(sys.argv[3])
results = {name: [] for name in editors}
for i in range(rounds):
    order = list(editors.items())
    for name, (binary, runtime) in order if i % 2 == 0 else order[::-1]:
        results[name].append(run(binary, runtime, mode))
for name, runs in results.items():
    print(name, "  ".join(
        f"{k}={statistics.median(r[k] for r in runs):.1f} [{min(r[k] for r in runs):.1f}-{max(r[k] for r in runs):.1f}]"
        for k in runs[0]))
