import unittest

from invoice import (
    UnknownRate,
    billable_hours,
    invoice_total,
    line_total,
    money,
    rate_for,
    render_line,
)

BILLABLE = {"date": "2026-09-21", "task": "landing page", "code": "dev", "minutes": 25}
NONBILLABLE = {"date": "2026-09-21", "task": "standup", "code": "admin", "minutes": 20, "nonbillable": True}


class HouseRules(unittest.TestCase):
    def test_money_rounds_half_up(self):
        self.assertEqual(money(2.675), 2.68)
        self.assertEqual(line_total(BILLABLE), 46.13)

    def test_hours_round_up_to_six_minute_blocks(self):
        self.assertEqual(billable_hours(1), 0.1)
        self.assertEqual(billable_hours(60), 1.0)
        self.assertEqual(billable_hours(61), 1.1)

    def test_nonbillable_is_shown_but_never_totalled(self):
        self.assertEqual(invoice_total([BILLABLE, NONBILLABLE]), invoice_total([BILLABLE]))
        self.assertIn("standup", render_line(NONBILLABLE))

    def test_unknown_code(self):
        with self.assertRaises(UnknownRate):
            rate_for("design")
