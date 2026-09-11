from typing import Protocol

from openai import AsyncOpenAI
from pydantic import BaseModel

from app.config import settings


class LLM(Protocol):
    async def complete_json[T: BaseModel](self, system: str, user: str, schema: type[T]) -> T: ...
    async def complete_text(self, system: str, user: str) -> str: ...
    async def embed(self, texts: list[str]) -> list[list[float]]: ...


class DeepInfraLLM:
    def __init__(self) -> None:
        self.client = AsyncOpenAI(
            base_url=settings.llm_base_url, api_key=settings.deepinfra_api_key
        )
        self.model = settings.llm_model
        self.embed_model = settings.embed_model

    async def complete_json[T: BaseModel](self, system: str, user: str, schema: type[T]) -> T:
        resp = await self.client.chat.completions.create(
            model=self.model,
            messages=[{"role": "system", "content": system}, {"role": "user", "content": user}],
            response_format={
                "type": "json_schema",
                "json_schema": {
                    "name": schema.__name__,
                    "strict": True,
                    "schema": schema.model_json_schema(),
                },
            },
            extra_body={"reasoning_effort": "low"},
        )
        return schema.model_validate_json(resp.choices[0].message.content or "{}")

    async def complete_text(self, system: str, user: str) -> str:
        resp = await self.client.chat.completions.create(
            model=self.model,
            messages=[{"role": "system", "content": system}, {"role": "user", "content": user}],
            extra_body={"reasoning_effort": "low"},
        )
        return resp.choices[0].message.content or ""

    async def embed(self, texts: list[str]) -> list[list[float]]:
        resp = await self.client.embeddings.create(model=self.embed_model, input=texts)
        return [d.embedding for d in resp.data]
