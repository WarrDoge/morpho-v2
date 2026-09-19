import unittest


def inv():
    import invoice

    return invoice


LINES = [
    {"date": "2026-09-21", "task": "landing page", "code": "dev", "minutes": 25},
    {"date": "2026-09-22", "task": "landing page", "code": "dev", "minutes": 61},
    {"date": "2026-09-22", "task": "standup", "code": "admin", "minutes": 20, "nonbillable": True},
]


class Task3(unittest.TestCase):
    def test_hours_add_up_per_task(self):
        self.assertEqual(inv().summary(LINES)["landing page"], 1.6)

    def test_every_task_appears(self):
        self.assertEqual(inv().summary(LINES)["standup"], 0.4)

    def test_nothing_to_summarize(self):
        self.assertEqual(inv().summary([]), {})
