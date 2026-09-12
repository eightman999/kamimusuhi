"""Shared fixtures: a deterministic fake runtime dir + helpers.

``make_runtime_dir`` builds the fixture tree the guard inspects:

    runtime/
      model/model.bin            fake weights
      model/revision.txt         e.g. a HuggingFace revision id
      tokenizer/tokenizer.json
      prompts/system_prompt.txt
      prompts/prompt_template.txt
      adapters/lora.bin          optional
      config/runtime.json        names / sampling / versions / ops config
      config/tools.json
      deps.lock

All content is deterministic (fixed literals + hash-derived bytes), so
rebuilt fixtures are byte-identical.
"""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

import pytest

from experiments.fi3.identity.diff import diff_manifests
from experiments.fi3.identity.manifest import (
    collect_manifest,
    runtime_identity_hash,
)
from experiments.fi3.policies.compatibility import classify_changes


def _payload(name: str) -> bytes:
    """Deterministic pseudo-binary content for fixture files."""
    return (
        f"FI3-FIXTURE:{name}\n".encode()
        + hashlib.sha256(name.encode()).hexdigest().encode()
        + b"\n"
    )


MODEL_V1 = _payload("model-weights-v1")
MODEL_V2 = _payload("model-weights-v2")
ADAPTER_V1 = _payload("adapter-lora-v1")
ADAPTER_V2 = _payload("adapter-lora-v2")

TOKENIZER_V1 = json.dumps(
    {
        "tokenizer": "kami-tok-v1",
        "vocab": {"<s>": 0, "</s>": 1, "hello": 2, "world": 3},
        "merges": ["h e", "he l"],
    },
    sort_keys=True,
    indent=2,
).encode()

TOOLS_V1 = json.dumps(
    {
        "schema": "kami-tools/2.1.0",
        "tools": [
            {"name": "search", "args": {"q": "str"}},
            {"name": "read_file", "args": {"path": "str"}},
        ],
    },
    sort_keys=True,
    indent=2,
)

DEPS_V1 = "numpy==2.1.3\npytest==8.3.4\n"
REVISION_V1 = "hf-rev-0001"

SYSTEM_PROMPT_V1 = (
    "You are Kamimusuhi, a careful research assistant.\n"
    "Answer precisely and cite your sources.\n"
)
PROMPT_TEMPLATE_V1 = "[INST] {{ user_input }} [/INST]\n"

BASELINE_CONFIG = {
    "model": {"name": "kami-fake-7b"},
    "tokenizer": {"name": "kami-tok-v1"},
    "sampling": {
        "temperature": 0.7,
        "top_p": 0.9,
        "top_k": 40,
        "seed": 1234,
        "repetition_penalty": 1.05,
        "context_length": 4096,
    },
    "versions": {
        "runtime": "0.3.1",
        "memory_schema": "1.0.0",
        "tool_schema": "2.1.0",
    },
    "operational": {"log_dir": "logs/main", "ui_theme": "dark"},
}


def make_runtime_dir(
    root,
    *,
    config=None,
    model_bytes: bytes = MODEL_V1,
    tokenizer_bytes: bytes = TOKENIZER_V1,
    system_prompt: str = SYSTEM_PROMPT_V1,
    prompt_template: str = PROMPT_TEMPLATE_V1,
    revision: str = REVISION_V1,
    adapter_bytes: bytes = ADAPTER_V1,
    with_adapter: bool = True,
    tools: str = TOOLS_V1,
    deps: str = DEPS_V1,
) -> Path:
    """Create a complete fixture runtime dir under *root*."""
    root = Path(root)
    for d in ("model", "tokenizer", "prompts", "adapters", "config"):
        (root / d).mkdir(parents=True, exist_ok=True)
    (root / "model/model.bin").write_bytes(model_bytes)
    (root / "model/revision.txt").write_text(revision + "\n", encoding="utf-8")
    (root / "tokenizer/tokenizer.json").write_bytes(tokenizer_bytes)
    (root / "prompts/system_prompt.txt").write_text(
        system_prompt, encoding="utf-8"
    )
    (root / "prompts/prompt_template.txt").write_text(
        prompt_template, encoding="utf-8"
    )
    if with_adapter:
        (root / "adapters/lora.bin").write_bytes(adapter_bytes)
    cfg = copy.deepcopy(config) if config is not None else copy.deepcopy(BASELINE_CONFIG)
    (root / "config/runtime.json").write_text(
        json.dumps(cfg, indent=2), encoding="utf-8"
    )
    (root / "config/tools.json").write_text(tools, encoding="utf-8")
    (root / "deps.lock").write_text(deps, encoding="utf-8")
    return root


def edit_config(runtime_dir, fn):
    """Load runtime.json, apply ``fn(cfg)`` in place, write it back."""
    p = Path(runtime_dir) / "config/runtime.json"
    cfg = json.loads(p.read_text(encoding="utf-8"))
    fn(cfg)
    p.write_text(json.dumps(cfg, indent=2), encoding="utf-8")
    return cfg


def classify_runtime(baseline_manifest, runtime_dir):
    """(report, changes, current_manifest) vs a stored baseline."""
    current = collect_manifest(runtime_dir)
    changes = diff_manifests(baseline_manifest, current)
    return classify_changes(changes), changes, current


def components_of(manifest):
    return manifest["components"]


@pytest.fixture
def runtime_dir(tmp_path):
    return make_runtime_dir(tmp_path / "runtime")


@pytest.fixture
def baseline_manifest(runtime_dir):
    return collect_manifest(runtime_dir)


@pytest.fixture
def baseline_identity(baseline_manifest):
    return runtime_identity_hash(baseline_manifest)


@pytest.fixture
def state_dir(tmp_path):
    return tmp_path / "fi3-state"
