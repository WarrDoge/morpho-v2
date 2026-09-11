"""score = relevance × importance × recency × confidence × reinforcement (SPEC §15)."""

import math
from datetime import UTC, datetime
from typing import Any


def recency_factor(row: dict[str, Any], now: datetime, half_life_days: float) -> float:
    ref = row.get("last_reinforced_at") or row["created_at"]
    age_days = max(0, int((now - ref).total_seconds() // 86400))  # whole days: replayable
    return math.exp(-age_days / half_life_days)


def score(
    row: dict[str, Any],
    relevance: float,
    now: datetime | None = None,
    half_life_days: float = 30,
) -> float:
    now = now or datetime.now(UTC)
    importance = row.get("importance", 1.0)
    confidence = row.get("confidence", 1.0)
    reinforcement = 1 + math.log1p(row.get("access_count", 0))
    return (
        relevance
        * importance
        * recency_factor(row, now, half_life_days)
        * confidence
        * reinforcement
    )


def decayed_importance(row: dict[str, Any], now: datetime, half_life_days: float) -> float:
    return row["importance"] * recency_factor(row, now, half_life_days)
