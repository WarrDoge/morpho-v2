import unittest


def fmt(seconds):
    from durations import format_duration

    return format_duration(seconds)


def parse(text):
    from durations import parse_duration

    return parse_duration(text)


class Task3(unittest.TestCase):
    def test_hours_and_minutes(self):
        self.assertEqual(fmt(5400), "1h30m")

    def test_seconds(self):
        self.assertEqual(fmt(45), "45s")

    def test_whole_hours(self):
        self.assertEqual(fmt(7200), "2h")

    def test_all_units(self):
        self.assertEqual(fmt(3723), "1h2m3s")

    def test_zero(self):
        self.assertEqual(fmt(0), "0s")

    def test_round_trip(self):
        for n in [1, 59, 60, 61, 3599, 3600, 86399, 90061]:
            self.assertEqual(parse(fmt(n)), n, n)
