import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("mock-hubstaff.py")
STATE_ENV = "LBX_MOCK_HUBSTAFF_STATE"


class MockHubstaffTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="lbx-mock-hubstaff-")
        self.addCleanup(self.temp.cleanup)
        self.state = Path(self.temp.name) / "state.json"
        self.env = os.environ.copy()
        self.env[STATE_ENV] = str(self.state)

    def run_script(self, *arguments, env=None):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *arguments],
            env=env if env is not None else self.env,
            check=False,
            capture_output=True,
            text=True,
        )

    def test_help_is_available_without_a_state_file(self):
        result = self.run_script("--help", env={})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(STATE_ENV, result.stdout)
        self.assertIn("set running", result.stdout)
        self.assertIn("set stopped", result.stdout)
        self.assertIn("set unavailable", result.stdout)
        self.assertIn("start, stop, launch, and resume", result.stdout)

    def test_every_non_help_command_requires_an_explicit_state_path(self):
        result = self.run_script("send", "status", env={})
        self.assertEqual(result.returncode, 2)
        self.assertIn(STATE_ENV, result.stderr)
        self.assertFalse(self.state.exists())

        result = self.run_script("set", "running", env={})
        self.assertEqual(result.returncode, 2)
        self.assertIn(STATE_ENV, result.stderr)
        self.assertFalse(self.state.exists())

    def test_missing_state_defaults_to_stopped_without_writing_any_file(self):
        readiness = self.run_script("send", "wait-login")
        status = self.run_script("send", "status")
        self.assertEqual(readiness.returncode, 0, readiness.stderr)
        self.assertEqual(status.returncode, 0, status.stderr)
        self.assertEqual(json.loads(readiness.stdout)["network_online"], False)
        snapshot = json.loads(status.stdout)
        self.assertFalse(snapshot["tracking"])
        self.assertEqual(
            snapshot["active_project"]["name"], "Mock Hubstaff project"
        )
        self.assertTrue(snapshot["active_project"]["id"].startswith("mock-"))
        self.assertFalse(self.state.exists())

    def test_set_states_affect_adapter_responses_and_read_commands_are_read_only(self):
        running = self.run_script("set", "running")
        self.assertEqual(running.returncode, 0, running.stderr)
        before_reads = self.state.read_bytes()

        readiness = self.run_script("send", "wait-login")
        status = self.run_script("send", "status")
        self.assertEqual(readiness.returncode, 0, readiness.stderr)
        self.assertEqual(status.returncode, 0, status.stderr)
        self.assertTrue(json.loads(status.stdout)["tracking"])
        self.assertEqual(self.state.read_bytes(), before_reads)
        self.assertEqual(list(self.state.parent.glob(".state.json.*.tmp")), [])

        stopped = self.run_script("set", "stopped")
        self.assertEqual(stopped.returncode, 0, stopped.stderr)
        self.assertFalse(json.loads(self.run_script("send", "status").stdout)["tracking"])

        unavailable = self.run_script("set", "unavailable")
        self.assertEqual(unavailable.returncode, 0, unavailable.stderr)
        for args in (("send", "wait-login"), ("send", "status")):
            result = self.run_script(*args)
            self.assertEqual(result.returncode, 2)
            self.assertIn("unavailable", result.stderr.lower())

        logged_out = self.run_script("set", "logged-out")
        self.assertEqual(logged_out.returncode, 0, logged_out.stderr)
        self.assertFalse(
            json.loads(self.run_script("send", "wait-login").stdout)["logged_in"]
        )

    def test_malformed_state_fails_clearly(self):
        self.state.write_text("{broken", encoding="utf-8")
        result = self.run_script("send", "status")
        self.assertEqual(result.returncode, 2)
        self.assertIn("malformed", result.stderr.lower())
        self.assertNotIn("Traceback", result.stderr)

    def test_real_style_control_commands_are_rejected_without_state_changes(self):
        self.assertEqual(self.run_script("set", "stopped").returncode, 0)
        before = self.state.read_bytes()
        for action in ("start", "stop", "launch", "resume"):
            result = self.run_script("send", action)
            self.assertEqual(result.returncode, 2)
            self.assertIn("unsupported", result.stderr.lower())
        self.assertEqual(self.state.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
