#!/usr/bin/env python3
"""Real-provider mission against a fresh disposable Git repo; incurs API usage.

Uses existing Maestro credentials. No fake servers or canned model responses.
Preserves the repository, event database, and report for inspection.
"""
import argparse
import json
import pathlib
import shutil
import sqlite3
import subprocess
import tempfile
import time
import urllib.request

OBJECTIVE = """Fix this Python stats utility. Create exactly four tasks: Designer, then Coder,
then independent Reviewer and Security tasks. The runtime handles integration
after their gates; do not schedule an extra integration-confirmation task.
Designer: read README.md and stats.py, then provide an implementation contract.
Coder: run check 0 BEFORE edits and observe the failure, fix mean, add median
(odd/even/empty), replace eval with safe JSON parsing as specified in README,
and add regression tests for each requirement including malicious input rejection.
Reviewer and Security: independently inspect the changed code and run check 0;
provide actual evidence and a verdict. Parsing is high risk. Only modify stats.py
and tests/**. Finish the reviewed, tested, integrated mission."""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=shutil.which("maestro") or "maestro")
    parser.add_argument("--config", type=pathlib.Path, default=pathlib.Path.home() / ".config/maestro/maestro.toml")
    parser.add_argument("--timeout", type=int, default=900)
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    root = pathlib.Path(tempfile.mkdtemp(prefix="maestro-foundry-live-"))
    (root / "tests").mkdir()
    (root / ".gitignore").write_text(".maestro/\n__pycache__/\n*.pyc\nsmoke-report.json\n")
    (root / "stats.py").write_text('''def mean(values):
    if not values:
        raise ValueError("empty input")
    return sum(values) / (len(values) + 1)


def parse_numbers(text):
    return eval(text)
''')
    (root / "tests/test_stats.py").write_text('''import unittest
from stats import mean, parse_numbers

class StatsTests(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([2, 4, 6]), 4)
    def test_empty_mean(self):
        with self.assertRaises(ValueError):
            mean([])
    def test_parse_numbers(self):
        self.assertEqual(parse_numbers("[1, 2.5, -3]"), [1, 2.5, -3])
''')
    (root / "README.md").write_text("""# Stats utility
Python standard library only. Public API: mean(values), parse_numbers(text).
Configured check 0: /usr/bin/python -m unittest discover -s tests -v.
Need median(values), correct averages, and safe parsing of a JSON array of finite numbers.
Reject booleans, non-list JSON, nested lists, strings and non-finite numbers with ValueError.
Empty median must raise ValueError. Never execute code from input.
Only modify stats.py and tests/**.
""")
    # The published Foundry template uses inline checks and explicit finalApproval.
    config = args.config.read_text()
    if "checks = []" not in config or "finalApproval = true" not in config:
        raise SystemExit("Use the published Foundry template with checks=[] and finalApproval=true.")
    config = config.replace("checks = []", 'checks = [{name = "unit tests", command = ["/usr/bin/python", "-m", "unittest", "discover", "-s", "tests", "-v"]}]')
    config = config.replace("finalApproval = true", "finalApproval = false")
    (root / "maestro.toml").write_text(config)
    for cmd in [["git", "init", "-q"], ["git", "add", "."], ["git", "-c", "user.name=Maestro Smoke", "-c", "user.email=smoke@localhost", "commit", "-qm", "Seed failing mean and unsafe parser"]]:
        subprocess.run(cmd, cwd=root, check=True)
    check = ["/usr/bin/python", "-m", "unittest", "discover", "-s", "tests", "-v"]
    baseline = subprocess.run(check, cwd=root, capture_output=True, text=True)
    assert baseline.returncode == 1, baseline.stderr
    print(f"Repository: {root}\nBaseline: failing as expected", flush=True)

    def cli(*cmd):
        result = subprocess.run([binary, "--cwd", str(root), "--json", *cmd], capture_output=True, text=True, timeout=60, check=True)
        return json.loads(result.stdout)

    mid = cli("submit", OBJECTIVE)["missionId"]
    descriptor = json.loads((root / ".maestro/daemon.json").read_text())
    def detail():
        with urllib.request.urlopen(descriptor["url"] + "/api/missions/" + mid, timeout=10) as response:
            return json.load(response)

    deadline = time.monotonic() + args.timeout
    previous = None
    while time.monotonic() < deadline:
        state = detail()
        summary = (state["mission"]["status"], tuple((t["role"], t["status"]) for t in state["tasks"]))
        if summary != previous:
            print(summary, flush=True)
            previous = summary
        if summary[0] in {"completed", "failed", "blocked", "cancelled", "awaiting_approval"}:
            break
        time.sleep(5)
    else:
        cli("cancel", mid)
        raise SystemExit(f"Mission timed out and was cancelled: {root}")

    final = subprocess.run(check, cwd=root, capture_output=True, text=True)
    with sqlite3.connect(root / ".maestro/maestro.db") as db:
        db.row_factory = sqlite3.Row
        invocations = [dict(row) for row in db.execute("SELECT role, provider_id, model, status, input_tokens, output_tokens, cost_micro_usd, latency_ms, error FROM invocations")]
        events = [dict(row) for row in db.execute("SELECT seq, task_id, kind, payload FROM events ORDER BY seq")]
    report = {"root": str(root), "missionId": mid, "detail": state, "baseline": baseline.stderr, "final": final.stderr, "invocations": invocations, "events": events}
    (root / "smoke-report.json").write_text(json.dumps(report, indent=2))
    print(f"Report: {root / 'smoke-report.json'}", flush=True)
    if state["mission"]["status"] != "completed" or final.returncode:
        raise SystemExit("Live mission did not complete successfully; inspect the report.")
    assert {i["role"] for i in invocations if i["status"] == "succeeded"} == {"planner", "designer", "coder", "reviewer", "security"}
    assert all(i["provider_id"] != "mock" for i in invocations)
    print(final.stderr, flush=True)


if __name__ == "__main__":
    main()
