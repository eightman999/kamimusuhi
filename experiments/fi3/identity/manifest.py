"""Runtime manifest collection.

A *runtime dir* is the deployable unit Fi3 guards.  The fixture layout
used by tests (and the CLIs) is::

    runtime/
      model/model.bin            # weights; may be a symlink (silent-pull vector)
      model/revision.txt         # model revision, e.g. a HuggingFace rev id
      tokenizer/tokenizer.json
      prompts/system_prompt.txt
      prompts/prompt_template.txt
      adapters/lora.bin          # optional; absence is recorded
      config/runtime.json        # names, sampling params, version strings, ops config
      config/tools.json          # tool schema definition
      deps.lock                  # dependency lockfile

``collect_manifest`` turns that tree into a manifest::

    {
      "manifest_version": 1,
      "components": { ... every guarded field ... },
    }

``runtime_identity_hash`` hashes the canonical serialization of the
whole manifest (manifest_version included), giving one deterministic
identity per runtime configuration.

Missing *required* files produce ``None`` fields rather than an
exception, so a post-baseline deletion shows up as a diff.  A baseline
with missing required components is rejected by
:func:`missing_required_fields` (verify CLI refuses to initialize from
an incomplete runtime).
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

from .canonical import canonical_hash, canonical_text_hash, flatten_components
from .hashing import fingerprint

MANIFEST_VERSION = 1

# Runtime-dir layout (relative paths).  All are required except the
# adapter, which is optional-but-recorded.
LAYOUT = {
    "model_file": "model/model.bin",
    "model_revision_file": "model/revision.txt",
    "tokenizer_file": "tokenizer/tokenizer.json",
    "system_prompt_file": "prompts/system_prompt.txt",
    "prompt_template_file": "prompts/prompt_template.txt",
    "adapter_file": "adapters/lora.bin",
    "config_file": "config/runtime.json",
    "tool_schema_file": "config/tools.json",
    "dependency_lock_file": "deps.lock",
}

OPTIONAL_FILES = {"adapter_file"}

# Every guarded component field, as a nested template with default
# values.  This is the single source of truth for (a) manifest shape,
# (b) the policy-completeness check, (c) canonical field order tests.
COMPONENT_SCHEMA: Dict[str, Any] = {
    "model": {
        "name": None,
        "hash": None,
        "revision": None,
        "link": None,
    },
    "tokenizer": {
        "name": None,
        "hash": None,
        "link": None,
    },
    "system_prompt": {
        "hash": None,
        "canonical_hash": None,
    },
    "prompt_template": {
        "hash": None,
        "canonical_hash": None,
    },
    "adapter": {
        "present": None,
        "hash": None,
        "link": None,
    },
    "sampling": {
        "temperature": None,
        "top_p": None,
        "top_k": None,
        "seed": None,
        "repetition_penalty": None,
        "context_length": None,
    },
    "versions": {
        "runtime": None,
        "memory_schema": None,
        "tool_schema": None,
    },
    "tool_schema": {
        "hash": None,
    },
    "dependency_lock": {
        "hash": None,
    },
    "operational": {
        "log_dir": None,
        "ui_theme": None,
    },
}

# Flattened field names that must be non-None for a usable baseline.
REQUIRED_FIELDS = [
    "model.name",
    "model.hash",
    "model.revision",
    "tokenizer.name",
    "tokenizer.hash",
    "system_prompt.hash",
    "system_prompt.canonical_hash",
    "prompt_template.hash",
    "prompt_template.canonical_hash",
    "adapter.present",
    "sampling.temperature",
    "sampling.top_p",
    "sampling.top_k",
    "sampling.seed",
    "sampling.repetition_penalty",
    "sampling.context_length",
    "versions.runtime",
    "versions.memory_schema",
    "versions.tool_schema",
    "tool_schema.hash",
    "dependency_lock.hash",
]

# runtime.json sections pulled into the manifest.
_SAMPLING_KEYS = (
    "temperature",
    "top_p",
    "top_k",
    "seed",
    "repetition_penalty",
    "context_length",
)
_VERSION_KEYS = ("runtime", "memory_schema", "tool_schema")
_OPERATIONAL_KEYS = ("log_dir", "ui_theme")


class ManifestError(Exception):
    """Raised when the runtime dir cannot be interpreted at all."""


def _read_text(path: Path) -> Optional[str]:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None


def _text_hashes(path: Path) -> Dict[str, Optional[str]]:
    """Raw-byte hash + canonicalized-text hash of a prompt file."""
    fp = fingerprint(path)
    raw = fp.sha256
    text = _read_text(path) if fp.exists else None
    canon = canonical_text_hash(text) if text is not None else None
    return {"hash": raw, "canonical_hash": canon}


def _load_config(runtime_dir: Path) -> Dict[str, Any]:
    cfg_path = runtime_dir / LAYOUT["config_file"]
    try:
        raw = json.loads(cfg_path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, json.JSONDecodeError) as e:
        raise ManifestError(f"cannot parse {cfg_path}: {e}") from e
    if not isinstance(raw, dict):
        raise ManifestError(f"{cfg_path} must contain a JSON object")
    return raw


def _section(cfg: Dict[str, Any], name: str, keys) -> Dict[str, Any]:
    sec = cfg.get(name)
    if not isinstance(sec, dict):
        return {k: None for k in keys}
    return {k: sec.get(k) for k in keys}


def collect_manifest(runtime_dir: Union[str, Path]) -> Dict[str, Any]:
    """Collect a manifest from a runtime dir.

    Never raises for missing component files (they become ``None``);
    raises :class:`ManifestError` only when the runtime config exists but
    is unparseable.
    """
    runtime_dir = Path(runtime_dir)
    cfg = _load_config(runtime_dir)

    model_fp = fingerprint(runtime_dir / LAYOUT["model_file"])
    tok_fp = fingerprint(runtime_dir / LAYOUT["tokenizer_file"])
    adapter_fp = fingerprint(runtime_dir / LAYOUT["adapter_file"])
    tool_fp = fingerprint(runtime_dir / LAYOUT["tool_schema_file"])
    lock_fp = fingerprint(runtime_dir / LAYOUT["dependency_lock_file"])

    revision = _read_text(runtime_dir / LAYOUT["model_revision_file"])
    model_cfg = cfg.get("model") if isinstance(cfg.get("model"), dict) else {}
    tok_cfg = (
        cfg.get("tokenizer") if isinstance(cfg.get("tokenizer"), dict) else {}
    )

    components = {
        "model": {
            "name": model_cfg.get("name"),
            "hash": model_fp.sha256,
            "revision": revision.strip() if revision is not None else None,
            "link": model_fp.link_target,
        },
        "tokenizer": {
            "name": tok_cfg.get("name"),
            "hash": tok_fp.sha256,
            "link": tok_fp.link_target,
        },
        "system_prompt": _text_hashes(runtime_dir / LAYOUT["system_prompt_file"]),
        "prompt_template": _text_hashes(
            runtime_dir / LAYOUT["prompt_template_file"]
        ),
        "adapter": {
            "present": adapter_fp.exists,
            "hash": adapter_fp.sha256,
            "link": adapter_fp.link_target,
        },
        "sampling": _section(cfg, "sampling", _SAMPLING_KEYS),
        "versions": _section(cfg, "versions", _VERSION_KEYS),
        "tool_schema": {"hash": tool_fp.sha256},
        "dependency_lock": {"hash": lock_fp.sha256},
        "operational": _section(cfg, "operational", _OPERATIONAL_KEYS),
    }

    # Unknown-but-present components must surface as fields, not be
    # dropped: schema keys not produced above default to None so the
    # completeness policy still covers them.
    for group, fields in COMPONENT_SCHEMA.items():
        for field in fields:
            components.setdefault(group, {}).setdefault(field, None)

    return {"manifest_version": MANIFEST_VERSION, "components": components}


def component_fields() -> List[str]:
    """All dotted component field names from :data:`COMPONENT_SCHEMA`."""
    return sorted(flatten_components(COMPONENT_SCHEMA))


def missing_required_fields(manifest: Dict[str, Any]) -> List[str]:
    """Required fields that are None/missing in *manifest*."""
    flat = flatten_components(manifest.get("components", {}))
    return [f for f in REQUIRED_FIELDS if flat.get(f) is None]


def runtime_identity_hash(manifest: Dict[str, Any]) -> str:
    """Deterministic identity: sha256 over the canonical manifest.

    Covers ``manifest_version`` plus every component field, so a manifest
    format bump also changes the identity.
    """
    payload = {
        "manifest_version": manifest.get("manifest_version"),
        "components": manifest.get("components", {}),
    }
    return canonical_hash(payload)
