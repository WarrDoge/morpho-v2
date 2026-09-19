import sys
import unittest


def entries():
    if "src" not in sys.path:
        sys.path.insert(0, "src")
    from timesheet import entries as module

    return module


def parse(line):
    return entries().parse_entry(line)


class Task3(unittest.TestCase):
    def test_total(self):
        lines = ["2026-09-18 09:00-10:30 review", "2026-09-18 11:00-11:20 standup"]
        self.assertEqual(entries().total_hours([parse(l) for l in lines]), 1.83)

    def test_empty(self):
        self.assertEqual(entries().total_hours([]), 0)
