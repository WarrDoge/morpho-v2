from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", extra="ignore")

    database_url: str = "postgresql://morpho:morpho@localhost:5432/morpho"
    deepinfra_api_key: str = ""
    llm_base_url: str = "https://api.deepinfra.com/v1/openai"
    llm_model: str = "zai-org/GLM-5.3-Flash"
    embed_model: str = "BAAI/bge-m3"
    embed_dim: int = 1024
    context_token_budget: int = 4000
    reflect_every_n_events: int = 10
    idle_reflect_seconds: float = 300
    worker_poll_seconds: float = 1.0
    worker_inprocess: bool = False
    memory_half_life_days: float = 30
    memory_archive_floor: float = 0.05
    max_proposals_per_cycle: int = 50
    dedupe_threshold: float = 0.95
    consumer_max_failures: int = 3


settings = Settings()
