import unittest


def inv():
    import invoice

    return invoice


LINES = [
    {"date": "2026-09-21", "task": "landing page", "code": "dev", "minutes": 25},
    {"date": "2026-09-21", "task": "standup", "code": "admin", "minutes": 20, "nonbillable": True},
    {"date": "2026-09-22", "task": "inbox", "code": "admin", "minutes": 40},
]


class Task2(unittest.TestCase):
    def test_one_day(self):
        self.assertEqual(inv().day_total(LINES, "2026-09-21"), 46.13)

    def test_another_day(self):
        self.assertEqual(inv().day_total(LINES, "2026-09-22"), 30.45)

    def test_a_day_with_nothing_on_it(self):
        self.assertEqual(inv().day_total(LINES, "2026-09-23"), 0.0)
