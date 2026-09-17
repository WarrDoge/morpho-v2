import sys
import unittest


def entries():
    if "src" not in sys.path:
        sys.path.insert(0, "src")
    from timesheet import entries as module

    return module


def parse(line):
    return entries().parse_entry(line)


class Task2(unittest.TestCase):
    def test_past_midnight(self):
        self.assertEqual(parse("2026-09-18 23:30-01:00 deploy")["minutes"], 90)

    def test_same_day_unchanged(self):
        self.assertEqual(parse("2026-09-18 08:00-17:15 onsite")["minutes"], 555)
