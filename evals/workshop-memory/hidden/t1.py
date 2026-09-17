import sys
import unittest


def entries():
    if "src" not in sys.path:
        sys.path.insert(0, "src")
    from timesheet import entries as module

    return module


def parse(line):
    return entries().parse_entry(line)


class Task1(unittest.TestCase):
    def test_fields(self):
        entry = parse("2026-09-18 09:00-10:30 review")
        self.assertEqual(
            (entry["date"], entry["start"], entry["end"], entry["minutes"], entry["task"]),
            ("2026-09-18", "09:00", "10:30", 90, "review"),
        )

    def test_minutes_are_int(self):
        self.assertIsInstance(parse("2026-09-18 09:00-09:45 standup")["minutes"], int)

    def test_task_of_several_words(self):
        self.assertEqual(parse("2026-09-18 13:00-13:45 code review")["task"], "code review")

    def test_malformed(self):
        for bad in [
            "",
            "2026-09-18 review",
            "2026-09-18 9-10 review",
            "18.09.2026 09:00-10:00 review",
            "2026-09-18 25:00-26:00 review",
            "2026-09-18 09:60-10:00 review",
            "2026-09-18 09:00-10:00",
        ]:
            with self.subTest(line=bad):
                self.assertRaises(ValueError, parse, bad)
