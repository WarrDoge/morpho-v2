import sys
import unittest


def entries():
    if "src" not in sys.path:
        sys.path.insert(0, "src")
    from timesheet import entries as module

    return module


def parse(line):
    return entries().parse_entry(line)


class Task4(unittest.TestCase):
    def test_summary(self):
        lines = [
            "2026-09-18 09:00-10:30 review",
            "2026-09-18 11:00-11:20 standup",
            "2026-09-18 14:00-14:40 review",
        ]
        self.assertEqual(entries().summarize([parse(l) for l in lines]), {"review": 2.17, "standup": 0.33})
