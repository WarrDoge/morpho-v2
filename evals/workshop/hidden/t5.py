import subprocess
import sys
import unittest


def fmt(seconds):
    from durations import format_duration

    return format_duration(seconds)


def parse(text):
    from durations import parse_duration

    return parse_duration(text)


class Task5(unittest.TestCase):
    def test_days(self):
        self.assertEqual(parse("2d"), 172800)

    def test_days_and_hours(self):
        self.assertEqual(parse("1d12h"), 129600)

    def test_format_day(self):
        self.assertEqual(fmt(86400), "1d")

    def test_format_all_units(self):
        self.assertEqual(fmt(90061), "1d1h1m1s")

    def test_round_trip(self):
        for n in [86399, 86400, 172859, 1000000]:
            self.assertEqual(parse(fmt(n)), n, n)

    def test_total_days(self):
        r = subprocess.run(
            [sys.executable, "total.py"], input="1d\n1d\n", capture_output=True, text=True, timeout=10
        )
        self.assertEqual(r.stdout.strip(), "2d")
