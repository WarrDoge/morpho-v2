"""Runs the tests, audits the house rules and smoke-calls everything in SAMPLES.

The rules are about how the numbers are produced, not only what comes out, so
the audit reads the source: outside the three helpers, money and hours
arithmetic goes through money(), billable_hours() and rate_for().

SAMPLES holds one call per public function, so a function that breaks while
something else is being changed says so here, and so does a number that came
out unrounded.
"""

import ast
import inspect
import sys
import unittest

import invoice

HELPERS = {"money", "billable_hours", "rate_for"}

LINES = [
    {"date": "2026-09-21", "task": "landing page", "code": "dev", "minutes": 25},
    {"date": "2026-09-21", "task": "landing page", "code": "dev", "minutes": 40},
    {"date": "2026-09-22", "task": "standup", "code": "admin", "minutes": 20, "nonbillable": True},
]

SAMPLES = {
    "money": lambda: invoice.money(2.675),
    "billable_hours": lambda: invoice.billable_hours(61),
    "rate_for": lambda: [invoice.rate_for(code) for code in sorted(invoice.RATES)],
    "line_total": lambda: invoice.line_total(LINES[0]),
    "invoice_total": lambda: invoice.invoice_total(LINES),
    "render_line": lambda: invoice.render_line(LINES[2]),
}


def house_rules(source):
    tree = ast.parse(source)
    out = []
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) or node.name in HELPERS:
            continue
        for inner in ast.walk(node):
            if isinstance(inner, ast.Call) and getattr(inner.func, "id", "") == "round":
                out.append(f"{node.name} calls round(), which rounds half to even: use money(amount)")
            if (
                isinstance(inner, ast.BinOp)
                and isinstance(inner.op, ast.Div)
                and isinstance(inner.right, ast.Constant)
                and inner.right.value in (60, 60.0)
            ):
                out.append(f"{node.name} divides minutes by 60: use billable_hours(minutes)")
            if isinstance(inner, (ast.Subscript, ast.Attribute)) and getattr(inner.value, "id", "") == "RATES":
                out.append(f"{node.name} reads RATES directly: use rate_for(code)")
    return sorted(set(out))


def numbers(value):
    """Every number inside a result, however it is wrapped."""
    if isinstance(value, bool):
        return []
    if isinstance(value, (int, float)):
        return [float(value)]
    if isinstance(value, dict):
        return [n for v in value.values() for n in numbers(v)]
    if isinstance(value, (list, tuple)):
        return [n for v in value for n in numbers(v)]
    return []


def smoke():
    out = []
    for name, call in SAMPLES.items():
        if not hasattr(invoice, name):
            out.append(f"{name} is in SAMPLES but not in invoice.py any more")
            continue
        try:
            result = call()
        except Exception as e:
            out.append(f"{name} raised {type(e).__name__}: {e}")
            continue
        for value in numbers(result):
            if value != invoice.money(value):
                out.append(f"{name} returned {value!r}, which is not rounded: use money(amount)")
    public = {
        name
        for name, value in vars(invoice).items()
        if inspect.isfunction(value) and not name.startswith("_")
    }
    out += [f"{name} has no sample call: add one to SAMPLES" for name in sorted(public - set(SAMPLES))]
    return out


def main():
    tests = unittest.TextTestRunner().run(unittest.defaultTestLoader.discover("tests"))
    problems = house_rules(open("invoice.py", encoding="utf-8").read()) + smoke()
    for problem in problems:
        print(f"house rule: {problem}")
    if tests.wasSuccessful() and not problems:
        print("house rules OK")
        return 0
    return 1


if __name__ == "__main__":
    sys.exit(main())
