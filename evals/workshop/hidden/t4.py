import subprocess
import sys
import unittest


def total(stdin):
    return subprocess.run(
        [sys.executable, "total.py"], input=stdin, capture_output=True, text=True, timeout=10
    )


class Task4(unittest.TestCase):
    def test_sums(self):
        r = total("1h\n30m\n")
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "1h30m"))

    def test_skips_blank_lines(self):
        r = total("\n45s\n\n15s\n")
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "1m"))

    def test_empty_input(self):
        r = total("")
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "0s"))

    def test_invalid_line(self):
        r = total("1h\nlater\n")
        self.assertEqual(r.returncode, 1)
        self.assertTrue(r.stderr.strip())

    def test_decimals(self):
        r = total("1.5h\n")
        self.assertEqual(r.stdout.strip(), "1h30m")
