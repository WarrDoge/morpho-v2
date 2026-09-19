import sys
import unittest


def entries():
    if "src" not in sys.path:
        sys.path.insert(0, "src")
    from timesheet import entries as module

    return module


def parse(line):
    return entries().parse_entry(line)


class Task6(unittest.TestCase):
    def test_surrounding_whitespace(self):
        entry = parse("  2026-09-18 09:00-10:30 review  \n")
        self.assertEqual((entry["date"], entry["minutes"], entry["task"]), ("2026-09-18", 90, "review"))

    def test_still_rejects(self):
        for bad in ["   ", "review 2026-09-18 09:00-10:30", " 2026-09-18 09:00 "]:
            with self.subTest(line=bad):
                self.assertRaises(ValueError, parse, bad)
