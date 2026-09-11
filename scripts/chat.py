"""Tiny REPL over the HTTP API. Usage: uv run python scripts/chat.py [http://localhost:8000]"""

import sys

import httpx

base = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8000"
with httpx.Client(base_url=base, timeout=120) as c:
    while True:
        try:
            text = input("> ").strip()
        except (EOFError, KeyboardInterrupt):
            break
        if not text:
            continue
        if text == "/cycle":
            print(c.post("/admin/cycle", params={"reflect": "true"}).json())
            continue
        r = c.post("/interact", json={"text": text}).json()
        print(r["response"])
        print(f"  [context tokens={r['context']['tokens']} mems={len(r['context']['memories'])}]")
