import unittest


def inv():
    import invoice

    return invoice


BILLABLE = {"date": "2026-09-21", "task": "landing page", "code": "dev", "minutes": 25}
NONBILLABLE = {"date": "2026-09-21", "task": "standup", "code": "admin", "minutes": 20, "nonbillable": True}


class HouseRules(unittest.TestCase):
    def test_money_rounds_half_up(self):
        self.assertEqual(inv().money(2.675), 2.68)
        self.assertEqual(inv().line_total(BILLABLE), 46.13)

    def test_hours_come_in_six_minute_blocks(self):
        self.assertEqual(inv().billable_hours(1), 0.1)
        self.assertEqual(inv().billable_hours(61), 1.1)

    def test_nonbillable_is_rendered_but_never_totalled(self):
        m = inv()
        self.assertEqual(m.invoice_total([BILLABLE, NONBILLABLE]), m.invoice_total([BILLABLE]))
        self.assertIn("standup", m.render_line(NONBILLABLE))

    def test_unknown_code_raises_unknown_rate(self):
        with self.assertRaises(inv().UnknownRate):
            inv().rate_for("design")


class Task1(unittest.TestCase):
    def test_loyalty_code_is_discounted(self):
        self.assertEqual(inv().rate_for("review"), 61.43)

    def test_other_codes_keep_the_full_rate(self):
        self.assertEqual(inv().rate_for("dev"), 92.25)
