"""Rebuild derived state from the transition log.

Usage: uv run python -m scripts.replay [up_to_transition_id]
"""

import asyncio
import sys

from app.services import build_services
from app.state.snapshots import replay


async def main() -> None:
    svc = await build_services()
    try:
        up_to = int(sys.argv[1]) if len(sys.argv) > 1 else None
        n = await replay(svc, up_to)
        print(f"applied {n} transitions")
    finally:
        await svc.close()


asyncio.run(main())
