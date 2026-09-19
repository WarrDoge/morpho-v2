import unittest

from durations import parse_duration


class ParseDuration(unittest.TestCase):
    def test_hours_and_minutes(self):
        self.assertEqual(parse_duration("1h30m"), 5400)

    def test_seconds(self):
        self.assertEqual(parse_duration("45s"), 45)

    def test_rejects_words(self):
        with self.assertRaises(ValueError):
            parse_duration("soon")
