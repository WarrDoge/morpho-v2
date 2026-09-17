import unittest

from timesheet.entries import parse_entry


class ParseEntryTest(unittest.TestCase):
    def test_minutes_and_task(self):
        entry = parse_entry("2026-09-18 09:00-10:30 review")
        self.assertEqual(entry["minutes"], 90)
        self.assertEqual(entry["task"], "review")

    def test_date_and_times(self):
        entry = parse_entry("2026-09-18 13:15-14:00 standup")
        self.assertEqual((entry["date"], entry["start"], entry["end"]), ("2026-09-18", "13:15", "14:00"))

    def test_malformed(self):
        with self.assertRaises(ValueError):
            parse_entry("yesterday 9-10 review")
