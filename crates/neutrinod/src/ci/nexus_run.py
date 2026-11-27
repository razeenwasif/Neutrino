"""Runs a Neutrino job's steps on a Nexus node.

Nexus runs one command per job; this is that command. It reads
.neutrino-run/job.json, runs each step in its own shell, and prints a marker
line before and after each one, which Neutrino reads back from the log to
show the steps:

    ##neutrino[start 0]
    ...the step's output...
    ##neutrino[end 0 0]        (index, exit code; "timeout" instead of a code)

Then it copies the job's artifacts into $DUET_OUTPUT_DIR/artifacts, which
Neutrino fetches with the job's outputs. Exits 1 if a step failed.
"""

import glob
import json
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
WINDOWS = os.name == "nt"


def marker(text):
    sys.stdout.write("##neutrino[" + text + "]\n")
    sys.stdout.flush()


GIT_BASH = (r"C:\Program Files\Git\bin\bash.exe", r"C:\Program Files (x86)\Git\bin\bash.exe")


def posix_shell(shell):
    """The bash or sh to run a step with, or None. On Windows that's Git for
    Windows' bash: System32\\bash.exe is WSL's launcher, which can't open a
    Windows path (it ran "C:UsersAlice..." and found nothing)."""
    if not WINDOWS:
        return shutil.which(shell)
    for guess in GIT_BASH:
        if os.path.exists(guess):
            return guess
    exe = shutil.which("bash") or shutil.which(shell)
    if exe and "\\system32\\" in exe.lower():
        return None
    return exe


def bash_path(path):
    """A path bash reads as written: Git's bash takes C:/... but not backslashes."""
    return path.replace("\\", "/") if WINDOWS else path


def shell_command(shell, path):
    """argv to run the script at `path` with `shell`."""
    if shell == "cmd":
        return ["cmd.exe", "/d", "/s", "/c", path]
    if shell in ("pwsh", "powershell"):
        exe = shutil.which(shell) or shutil.which("pwsh") or shutil.which("powershell") or shell
        return [exe, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path]
    if shell in ("bash", "sh"):
        exe = posix_shell(shell)
        if exe is None:
            raise RuntimeError(shell + " isn't installed on this node" + (" (install Git for Windows)" if WINDOWS else ""))
        return [exe, "-e", bash_path(path)]
    exe = shutil.which(shell)
    if exe is None:
        raise RuntimeError(shell + " isn't installed on this node")
    return [exe, path]


def default_shell():
    if posix_shell("bash"):
        return "bash"
    return "cmd" if WINDOWS else "sh"


def script_file(index, shell, text, cwd):
    ext = {"cmd": ".cmd", "pwsh": ".ps1", "powershell": ".ps1"}.get(shell, ".sh")
    path = os.path.join(HERE, "step-%d%s" % (index, ext))
    if shell == "cmd":
        body = "@echo off\r\ncd /d \"" + cwd + "\"\r\n" + text.replace("\n", "\r\n") + "\r\n"
    elif shell in ("pwsh", "powershell"):
        body = "$ErrorActionPreference = 'Stop'\nSet-Location -LiteralPath '" + cwd.replace("'", "''") + "'\n" + text + "\nexit $LASTEXITCODE\n"
    else:
        body = "cd '" + bash_path(cwd).replace("'", "'\\''") + "' || exit 1\n" + text + "\n"
    with open(path, "w", newline="") as f:
        f.write(body)
    return path


def run_step(step, failed):
    if not (step["run_if_failed"] if failed else step["run_if_ok"]):
        marker("skip %d" % step["index"])
        return None
    marker("start %d" % step["index"])
    shell = step.get("shell") or default_shell()
    cwd = os.path.join(ROOT, step.get("dir") or "")
    env = dict(os.environ)
    env.update(step["env"])
    env["NEUTRINO_WORKSPACE"] = ROOT
    try:
        argv = shell_command(shell, script_file(step["index"], shell, step["script"], cwd))
    except RuntimeError as e:
        print("##neutrino-note " + str(e))
        marker("end %d 127" % step["index"])
        return 127
    proc = subprocess.Popen(argv, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    deadline = time.time() + step["timeout_secs"]
    timed_out = False
    # Copy output through as it comes, line by line.
    while True:
        line = proc.stdout.readline()
        if line:
            sys.stdout.write(line.decode("utf-8", "replace"))
            sys.stdout.flush()
        elif proc.poll() is not None:
            break
        if time.time() > deadline:
            timed_out = True
            proc.kill()
            break
    code = proc.wait()
    if timed_out:
        marker("end %d timeout" % step["index"])
        return 124
    marker("end %d %d" % (step["index"], code))
    return code


def keep_artifacts(patterns):
    out = os.environ.get("DUET_OUTPUT_DIR")
    if not out or not patterns:
        return
    root_real = os.path.realpath(ROOT)
    dest = os.path.join(out, "artifacts")
    os.makedirs(dest, exist_ok=True)
    dest_real = os.path.realpath(dest)
    kept = 0
    for pattern in patterns:
        norm = pattern.replace("\\", "/")
        if os.path.isabs(pattern) or norm.startswith("/") or ".." in norm.split("/"):
            continue
        for match in glob.glob(os.path.join(ROOT, pattern), recursive=True):
            match_real = os.path.realpath(match)
            if match_real != root_real and not match_real.startswith(root_real + os.sep):
                continue
            rel = os.path.relpath(match_real, root_real)
            target = os.path.realpath(os.path.join(dest_real, rel))
            if target != dest_real and not target.startswith(dest_real + os.sep):
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            if os.path.isdir(match_real):
                shutil.copytree(match_real, target, dirs_exist_ok=True)
            else:
                shutil.copy2(match_real, target)
            kept += 1
    print("##neutrino-note kept %d artifact path%s" % (kept, "" if kept == 1 else "s"))


def main():
    with open(os.path.join(HERE, "job.json"), encoding="utf-8") as f:
        job = json.load(f)
    failed = False
    for step in job["steps"]:
        code = run_step(step, failed)
        if code not in (None, 0) and not step["continue_on_error"]:
            failed = True
    keep_artifacts(job.get("artifacts", []))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
