import unittest


def inv():
    import invoice

    return invoice


def invoice_text(lines):
    out = inv().render_invoice(lines)
    return out if isinstance(out, str) else "\n".join(out)


NONBILLABLE = {"date": "2026-09-21", "task": "standup", "code": "admin", "minutes": 20, "nonbillable": True}


class Task6(unittest.TestCase):
    def test_a_pasted_row_becomes_a_line(self):
        line = inv().parse_line("2026-09-21 dev 185 landing page")
        self.assertEqual(line["date"], "2026-09-21")
        self.assertEqual(line["code"], "dev")
        self.assertEqual(line["minutes"], 185)
        self.assertEqual(line["task"], "landing page")

    def test_nonsense_is_rejected(self):
        with self.assertRaises(ValueError):
            inv().parse_line("yesterday, some dev work")

    def test_an_invoice_from_pasted_rows(self):
        out = invoice_text(["2026-09-21 dev 25 landing page", NONBILLABLE])
        self.assertIn("landing page", out)
        self.assertIn("standup", out)
        self.assertIn("46.13", out)
