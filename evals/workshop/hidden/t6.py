import unittest


def parse(text):
    from durations import parse_duration

    return parse_duration(text)


class Task6(unittest.TestCase):
    def test_compact_form_still_parses(self):
        self.assertEqual(parse("1h30m"), 5400)

    def test_days_still_parse(self):
        self.assertEqual(parse("1d1s"), 86401)

    def test_garbage_still_rejected(self):
        self.assertRaises(ValueError, parse, "soon")
