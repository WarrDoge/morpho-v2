import unittest


def parse(text):
    from durations import parse_duration

    return parse_duration(text)


class Task2(unittest.TestCase):
    def test_decimal_hours(self):
        self.assertEqual(parse("1.5h"), 5400)

    def test_decimal_minutes(self):
        self.assertEqual(parse("0.5m"), 30)

    def test_quarter_minutes(self):
        self.assertEqual(parse("1.25m"), 75)

    def test_decimal_with_other_units(self):
        self.assertEqual(parse("1.5h30m"), 7200)

    def test_still_int(self):
        self.assertIsInstance(parse("1.5h"), int)
