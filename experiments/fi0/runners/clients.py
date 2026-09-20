"""Model client protocol and implementations.

``ModelClient`` — protocol: ``complete(prompt, task) -> ModelResponse``.

``MockModel`` (default, fully deterministic): parses the prompt back to a
``PromptContext`` and decides via ``task.expected(ctx)`` — a pure function
of *semantic content*.  Identical semantics → identical decisions across
all serializations.  This validates the harness and yields an honest
"stable on mock" result.

``SensitiveMockModel`` (positive control): decides as a deterministic
function of the RAW prompt bytes + seed — identical bytes → identical
decisions, but any serialization perturbation re-rolls the decision.  It
exists to prove the harness *can* detect drift; its drift is never
presented as real model drift.

``OpenAICompatibleClient``: real endpoint via ``FI_MODEL_BASE_URL`` /
``FI_MODEL_API_KEY`` / ``FI_MODEL_NAME`` env vars; plain urllib POST to
``{base}/chat/completions``.  Never used in tests.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import urllib.request
from dataclasses import dataclass
from typing import Optional, Protocol, runtime_checkable

from experiments.fi0.serializer.canonical import parse_prompt
from experiments.fi0.tasks.base import Task, extract_marker

TOKEN_RE = re.compile(r"\w+|[^\w\s]")


def approx_tokens(text: str) -> int:
    """Cheap deterministic token estimate (word/punct split)."""
    return len(TOKEN_RE.findall(text))


@dataclass(frozen=True)
class ModelResponse:
    text: str
    latency_ms: float       # simulated: deterministic function of prompt size
    prompt_chars: int
    prompt_tokens: int
    model: str


def _simulated_latency_ms(prompt: str) -> float:
    """Deterministic pseudo-latency: base + per-byte cost.

    Not wall-clock — a stable function of prompt size so latency is a
    real measured quantity of the serialization, reproducible across runs.
    """
    return round(4.0 + 0.02 * len(prompt.encode("utf-8")), 3)


@runtime_checkable
class ModelClient(Protocol):
    name: str

    def complete(self, prompt: str, task: Task) -> ModelResponse:
        ...


class MockModel:
    """Semantics-only deterministic model.

    The decision is ``task.expected(parse_prompt(prompt))``: a function of
    the parsed semantic content, never of the raw bytes.
    """

    name = "mock"

    def __init__(self, seed: int = 0):
        self.seed = seed  # recorded for provenance; decisions are seed-free

    def complete(self, prompt: str, task: Task) -> ModelResponse:
        ctx = parse_prompt(prompt)
        choice = task.expected(ctx)
        marker = extract_marker(ctx.identity, task.marker)
        text = f"[{marker}] ANSWER: {choice}"
        return ModelResponse(
            text=text,
            latency_ms=_simulated_latency_ms(prompt),
            prompt_chars=len(prompt),
            prompt_tokens=approx_tokens(prompt),
            model=self.name,
        )


class SensitiveMockModel:
    """Byte-sensitive control model.

    Choice = sha256(seed | raw_prompt_bytes) mod |options| — deliberately
    unstable under serialization perturbation.  Marker presence also
    depends on a hash bit, so persona consistency drifts too.
    """

    name = "sensitive-mock"

    def __init__(self, seed: int = 0):
        self.seed = seed

    def _hash(self, prompt: str) -> int:
        h = hashlib.sha256()
        h.update(f"fi0-sensitive|{self.seed}|".encode("utf-8"))
        h.update(prompt.encode("utf-8"))
        return int.from_bytes(h.digest(), "big")

    def complete(self, prompt: str, task: Task) -> ModelResponse:
        h = self._hash(prompt)
        choice = task.options[h % len(task.options)]
        marker = f"[{task.marker}] " if (h >> 8) & 1 else ""
        return ModelResponse(
            text=f"{marker}ANSWER: {choice}",
            latency_ms=_simulated_latency_ms(prompt),
            prompt_chars=len(prompt),
            prompt_tokens=approx_tokens(prompt),
            model=self.name,
        )


class OpenAICompatibleClient:
    """Optional real-model client (never required for tests).

    Reads FI_MODEL_BASE_URL / FI_MODEL_API_KEY / FI_MODEL_NAME.
    POSTs an OpenAI-style chat/completions payload with urllib only.
    """

    name = "openai-compatible"

    def __init__(self, seed: int = 0, timeout_s: float = 60.0):
        self.base_url = os.environ["FI_MODEL_BASE_URL"].rstrip("/")
        self.api_key = os.environ.get("FI_MODEL_API_KEY", "")
        self.model = os.environ.get("FI_MODEL_NAME", "default")
        self.seed = seed
        self.timeout_s = timeout_s

    def complete(self, prompt: str, task: Task) -> ModelResponse:
        body = {
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0,
            "seed": self.seed,
        }
        req = urllib.request.Request(
            f"{self.base_url}/chat/completions",
            data=json.dumps(body).encode("utf-8"),
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {self.api_key}",
            },
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=self.timeout_s) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
        text = payload["choices"][0]["message"]["content"]
        return ModelResponse(
            text=text,
            latency_ms=-1.0,  # wall latency not simulated for real clients
            prompt_chars=len(prompt),
            prompt_tokens=approx_tokens(prompt),
            model=self.model,
        )


def make_client(kind: str, seed: int = 0) -> ModelClient:
    if kind == "mock":
        return MockModel(seed)
    if kind in ("sensitive", "sensitive-mock"):
        return SensitiveMockModel(seed)
    if kind in ("openai", "openai-compatible"):
        return OpenAICompatibleClient(seed)
    raise ValueError(f"unknown model kind {kind!r}")
