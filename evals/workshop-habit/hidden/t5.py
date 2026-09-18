import unittest


def inv():
    import invoice

    return invoice


class Task5(unittest.TestCase):
    def test_a_quote_matches_the_line_it_would_become(self):
        m = inv()
        self.assertEqual(m.estimate(61, "dev"), m.line_total({"code": "dev", "minutes": 61}))
        self.assertEqual(m.estimate(61, "dev"), 101.48)

    def test_no_time_costs_nothing(self):
        self.assertEqual(inv().estimate(0, "dev"), 0.0)

    def test_an_unknown_code_still_raises(self):
        with self.assertRaises(inv().UnknownRate):
            inv().estimate(30, "design")
