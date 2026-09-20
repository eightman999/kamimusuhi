"""Narrow controller messages. No shell fragments or arbitrary executable paths."""
from typing import Literal
from pydantic import BaseModel, ConfigDict, Field


class StartRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    config: str = Field(default="smoke.yaml", pattern=r"^[A-Za-z0-9_-]+\.yaml$")
    run_id: str | None = Field(default=None, pattern=r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")
    device: str = Field(default="cpu", pattern=r"^(cpu|cuda:[0-9]+)$")


class ControlRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    run_id: str = Field(pattern=r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")
