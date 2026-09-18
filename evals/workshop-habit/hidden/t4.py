import unittest


def inv():
    import invoice

    return invoice


def invoice_text(lines):
    out = inv().render_invoice(lines)
    return out if isinstance(out, str) else "\n".join(out)


LINES = [
    {"date": "2026-09-21", "task": "landing page", "code": "dev", "minutes": 25},
    {"date": "2026-09-21", "task": "standup", "code": "admin", "minutes": 20, "nonbillable": True},
]


class Task4(unittest.TestCase):
    def test_every_line_is_rendered(self):
        out = invoice_text(LINES)
        self.assertIn("landing page", out)
        self.assertIn("standup", out)

    def test_total_row(self):
        out = invoice_text(LINES)
        self.assertIn("TOTAL", out)
        self.assertIn("46.13", out)

    def test_nonbillable_is_left_out_of_the_total(self):
        out = invoice_text(LINES)
        self.assertNotIn("63.53", out)
