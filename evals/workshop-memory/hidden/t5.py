import sys
import unittest


def entries():
    if "src" not in sys.path:
        sys.path.insert(0, "src")
    from timesheet import entries as module

    return module


def parse(line):
    return entries().parse_entry(line)


class Task5(unittest.TestCase):
    def test_csv_for_a_german_excel(self):
        import os
        import tempfile

        lines = ["2026-09-18 09:00-10:30 review", "2026-09-19 23:30-01:15 deploy"]
        path = os.path.join(tempfile.mkdtemp(), "out.csv")
        entries().export_csv([parse(l) for l in lines], path)
        with open(path, encoding="utf-8-sig") as f:
            rows = [[c.strip().strip('"') for c in l.split(";")] for l in f if "2026-09-1" in l]
        self.assertEqual(len(rows), 2)
        for row, (date, task, hours) in zip(rows, [("2026-09-18", "review", "1,5"), ("2026-09-19", "deploy", "1,75")]):
            self.assertIn(date, row)
            self.assertIn(task, row)
            self.assertTrue(hours in row or hours + "0" in row, row)
