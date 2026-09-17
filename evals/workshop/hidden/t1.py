import unittest


def parse(text):
    from durations import parse_duration

    return parse_duration(text)


class Task1(unittest.TestCase):
    def test_hours(self):
        self.assertEqual(parse("2h"), 7200)

    def test_minutes(self):
        self.assertEqual(parse("15m"), 900)

    def test_all_units(self):
        self.assertEqual(parse("1h2m3s"), 3723)

    def test_zero(self):
        self.assertEqual(parse("0s"), 0)

    def test_returns_int(self):
        self.assertIsInstance(parse("1h"), int)

    def test_empty(self):
        self.assertRaises(ValueError, parse, "")

    def test_bare_number(self):
        self.assertRaises(ValueError, parse, "5")

    def test_unknown_unit(self):
        self.assertRaises(ValueError, parse, "3x")

    def test_words(self):
        self.assertRaises(ValueError, parse, "one hour")
