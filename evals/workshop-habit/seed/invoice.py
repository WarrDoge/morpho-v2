"""Invoices for Priya's studio: rates, line totals and a rendered invoice.

House rules, kept by the tests in tests/ and audited by check.py:

* Money is rounded half up to two decimals. Not with round(), which rounds half
  to even: round(2.675, 2) is 2.67 where the accountant expects 2.68.
* Billable time rounds up to the next six-minute block, so 61 minutes bills as
  1.1 hours, not 1.0166.
* A line marked nonbillable is still shown on the invoice but adds nothing to
  any total.
* An unknown rate code raises UnknownRate, never KeyError.

A line is a dict: date, task, code, minutes, and optionally nonbillable.
"""

from decimal import ROUND_HALF_UP, Decimal

RATES = {"dev": 92.25, "review": 68.25, "admin": 43.50}


class UnknownRate(Exception):
    pass


def money(amount):
    """Round half up to two decimals."""
    return float(Decimal(str(amount)).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP))


def billable_hours(minutes):
    """Minutes as hours, rounded up to the next six-minute block."""
    return -(-int(minutes) // 6) / 10


def rate_for(code):
    if code not in RATES:
        raise UnknownRate(code)
    return RATES[code]


def line_total(line):
    return money(billable_hours(line["minutes"]) * rate_for(line["code"]))


def invoice_total(lines):
    return money(sum(line_total(l) for l in lines if not l.get("nonbillable")))


def render_line(line):
    mark = " (nonbillable)" if line.get("nonbillable") else ""
    return f"{line['task']}\t{billable_hours(line['minutes']):.1f}h\t{line_total(line):.2f}{mark}"
