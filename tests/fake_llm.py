"""Test double: queued JSON answers per schema name, bag-of-words embeddings."""

import hashlib
import math
import types
import typing
from collections import deque
from typing import Any

from pydantic import BaseModel

from app.config import settings


def _empty(annotation: Any) -> Any:
    origin = typing.get_origin(annotation)
    if origin is typing.Union or origin is types.UnionType:
        args = [a for a in typing.get_args(annotation) if a is not type(None)]
        return None if len(args) < len(typing.get_args(annotation)) else _empty(args[0])
    if origin is list or annotation is list:
        return []
    if origin is typing.Literal:
        return typing.get_args(annotation)[0]
    if isinstance(annotation, type) and issubclass(annotation, BaseModel):
        return empty_model(annotation)
    return {str: "", float: 0.0, int: 0, bool: False, dict: {}}.get(annotation)


def empty_model[T: BaseModel](schema: type[T]) -> T:
    return schema.model_validate(
        {name: _empty(f.annotation) for name, f in schema.model_fields.items()}
    )


def hash_embedding(text: str, dim: int = settings.embed_dim) -> list[float]:
    v = [0.0] * dim
    for tok in text.lower().split():
        h = int.from_bytes(hashlib.blake2b(tok.encode(), digest_size=4).digest(), "big")
        v[h % dim] += 1.0
    n = math.sqrt(sum(x * x for x in v)) or 1.0
    return [x / n for x in v]


class FakeLLM:
    def __init__(self) -> None:
        self.json: dict[str, deque[BaseModel]] = {}
        self.text: deque[str] = deque()
        self.errors: deque[Exception] = deque()
        self.calls: list[tuple[str, str]] = []

    def queue(self, *answers: BaseModel) -> None:
        for a in answers:
            self.json.setdefault(type(a).__name__, deque()).append(a)

    async def complete_json[T: BaseModel](self, system: str, user: str, schema: type[T]) -> T:
        self.calls.append((schema.__name__, user))
        if self.errors:
            raise self.errors.popleft()
        q = self.json.get(schema.__name__)
        if q:
            return typing.cast(T, q.popleft())
        return empty_model(schema)

    async def complete_text(self, system: str, user: str) -> str:
        self.calls.append(("text", user))
        return self.text.popleft() if self.text else "ok"

    async def embed(self, texts: list[str]) -> list[list[float]]:
        return [hash_embedding(t) for t in texts]
