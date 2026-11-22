#!/usr/bin/env python3
"""Drives programs in terse-host's real window (`--drive`, README "Driving
the window") with the scripts in scripts/drive-shots/, and keeps what they
show: each program's `shot` pictures and its report, in a folder of its own
under the output folder, for comparing one system's window with another's
(`.github/workflows/test.yml` runs it on Linux, macOS and Windows and
uploads the folder).

    scripts/drive-shots.py <terse-host> <out dir> [program ...]

Programs: hello, sparks, sheet (default: all three). Each runs with the
software renderer, scratch directories (inside the output folder) and no
relay or sound; its performance record (RECORD.md) is copied beside its
pictures. On Linux terse-host starts a headless weston of its own;
on Windows and macOS it refuses unless TERSE_DRIVE_DISPLAY=current, which
shows a window on this desktop: set it only on a machine whose desktop
nobody uses (a CI runner).

Each program ends as one of: `ok` (every expectation held), `failed` (an
expectation failed), `no gpu` (there was no graphics adapter: not counted
as a failure) or `error` (the drive did not finish: the host could not
start, open its window or read the script). The status is 1 if any
program failed or erred, else 0; a summary goes to standard output.
"""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts" / "drive-shots"
PROGRAMS = ["hello", "sparks", "sheet"]
SIZE = "960x720"


def outcome(status, out, err):
    """What a drive's exit status and output say."""
    if "no graphics adapter" in err:
        return "no gpu"
    if "drive: done, no expectation failed" in out and status == 0:
        return "ok"
    done = out.split("drive: done", 1)[1].splitlines()[0] if "drive: done" in out else ""
    if "failed" in done and not done.startswith(", no expectation"):
        return "failed"
    return "error"


def drive(host, out, name):
    d = out / name
    d.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="terse-drive-shots-") as t:
        return drive_in(host, d, name, pathlib.Path(t))


def drive_in(host, d, name, scratch):
    for s in ["store", "config", "data", "cache", "xdg-config", "xdg-data"]:
        (scratch / s).mkdir(parents=True, exist_ok=True)
    image = ROOT / "programs" / name / f"{name}.terse"
    script = SCRIPTS / f"{name}.txt"
    # The rule only records here: on a slow machine its notice would lie
    # over the pictures (the record is kept beside them all the same).
    args = [str(host), str(image), "--drive", str(script), "--renderer", "software", "--size", SIZE, "--consent", "deny",
            "--perf-rule", "off",
            "--store", str(scratch / "store"), "--config", str(scratch / "config"), "--data", str(scratch / "data")]
    env = dict(os.environ)
    env.update({"TERSE_AUDIO": "off", "XDG_CACHE_HOME": str(scratch / "cache"), "XDG_CONFIG_HOME": str(scratch / "xdg-config"),
                "XDG_DATA_HOME": str(scratch / "xdg-data")})
    env.pop("TERSE_DRIVE_NESTED", None)
    try:
        p = subprocess.run(args, cwd=d, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=600)
        status, o, e = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        status, o, e = -1, "", "timed out after 600 s\n"
    (d / "drive.log").write_text(f"$ {' '.join(args)}\nstatus {status}\n--- stdout\n{o}--- stderr\n{e}", encoding="utf-8")
    # The performance record the window kept (its `readings` lines say
    # what this system told the host about waiting for a CPU).
    for log in (scratch / "data" / "perf").glob("*.log"):
        shutil.copy(log, d / f"perf-{log.name}")
    shots = sorted(p.name for p in d.glob("*.png"))
    return outcome(status, o, e), shots, o, e


def main():
    if len(sys.argv) < 3:
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    host, out = pathlib.Path(sys.argv[1]).resolve(), pathlib.Path(sys.argv[2]).resolve()
    names = sys.argv[3:] or PROGRAMS
    for n in names:
        if n not in PROGRAMS:
            print(f"drive-shots: no script for {n} (scripts/drive-shots/<name>.txt)", file=sys.stderr)
            return 2
    bad = 0
    for n in names:
        result, shots, o, e = drive(host, out, n)
        print(f"{n}: {result}; pictures: {', '.join(shots) or 'none'}")
        if result in ("failed", "error"):
            bad += 1
            for line in (o + e).splitlines()[-30:]:
                print(f"  {line}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
