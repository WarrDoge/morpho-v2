"""Record/replay wrapper around any LLM. Caches every call; counts calls; strict forbids misses."""

import hashlib
import json
import re
from collections import Counter
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any

from pydantic import BaseModel

from app.context.composer import tokens
from app.llm import LLM

TIMESTAMP = re.compile(
    r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:[+-]\d{2}:\d{2}|Z)?"
)


class CacheMiss(Exception):
    pass


def _key(*parts: str) -> str:
    norm = "\x1f".join(TIMESTAMP.sub("<ts>", p) for p in parts)
    return hashlib.sha256(norm.encode()).hexdigest()


class RecordingLLM:
    def __init__(self, inner: LLM | None, path: Path, strict: bool = False) -> None:
        self.inner, self.path, self.strict = inner, path, strict
        self.cache: dict[str, Any] = json.loads(path.read_text()) if path.exists() else {}
        self.counts: Counter[str] = Counter()
        self.dirty = False

    async def _get(self, key: str, fetch: Callable[[], Awaitable[Any]], what: str) -> Any:
        if key in self.cache:
            return self.cache[key]
        if self.strict or self.inner is None:
            raise CacheMiss(f"call #{self.counts['llm_calls']} {what}")
        self.counts["cache_misses"] += 1
        self.cache[key] = val = await fetch()
        self.dirty = True
        self.save()
        return val

    async def complete_json[T: BaseModel](self, system: str, user: str, schema: type[T]) -> T:
        self.counts["llm_calls"] += 1
        self.counts["prompt_tokens"] += tokens(system + user)
        schema_json = json.dumps(schema.model_json_schema(), sort_keys=True)

        async def fetch() -> Any:
            assert self.inner is not None
            return (await self.inner.complete_json(system, user, schema)).model_dump(mode="json")

        key = _key("json", system, user, schema_json)
        what = f"json/{schema.__name__}:\n{user}"
        return schema.model_validate(await self._get(key, fetch, what))

    async def complete_text(self, system: str, user: str) -> str:
        self.counts["llm_calls"] += 1
        self.counts["prompt_tokens"] += tokens(system + user)

        async def fetch() -> Any:
            assert self.inner is not None
            return await self.inner.complete_text(system, user)

        what = f"text:\n{system}\n---\n{user}"
        return str(await self._get(_key("text", system, user), fetch, what))

    async def embed(self, texts: list[str]) -> list[list[float]]:
        self.counts["embed_calls"] += 1
        keys = [_key("embed", t) for t in texts]
        missing = [t for t, k in zip(texts, keys, strict=True) if k not in self.cache]
        if missing:
            if self.strict or self.inner is None:
                raise CacheMiss(f"embed #{self.counts['embed_calls']}: {missing[0][:200]!r}")
            self.counts["cache_misses"] += 1
            vecs = await self.inner.embed(missing)
            for t, v in zip(missing, vecs, strict=True):
                self.cache[_key("embed", t)] = [round(x, 6) for x in v]
            self.dirty = True
            self.save()
        return [self.cache[k] for k in keys]

    def save(self) -> None:
        if self.dirty:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self.path.write_text(json.dumps(self.cache, sort_keys=True, separators=(",", ":")))
            self.dirty = False
