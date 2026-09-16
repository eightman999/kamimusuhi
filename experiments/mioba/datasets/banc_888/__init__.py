"""BANC materialization 888 — source-file registry (AFC §3).

Declares which upstream files constitute the dataset, where they are
fetched from and where the local cache lives. Data payloads stay out of
git under ``data/cache/`` per repo convention; this module only carries
names, URLs and integrity expectations.
"""
from __future__ import annotations

import os
from pathlib import Path

DATASET_KIND = "banc_888"
MATERIALIZATION = 888
DATASET_PREFIX = "banc"

BASE_URL = ("https://storage.googleapis.com/"
            "lee-lab_brain-and-nerve-cord-fly-connectome/"
            "compiled_data/banc_888")

#: file name -> (bytes, required) — sizes observed on 2026-09-16;
#: used for sanity checks and progress reporting, not strict equality.
SOURCE_FILES = {
    "banc_888_meta.feather": (57_503_026, True),
    "banc_888_metrics.feather": (12_285_378, False),
    "banc_888_neurotransmitter_prediction_v2.csv": (21_107_592, False),
    "banc_888_edgelist_simple_v3.feather": (None, True),
}

PAPER = ("Bates, Phelps, Kim, Yang et al., 'Distributed control circuits "
         "across a brain-and-cord connectome', Nature 2026, "
         "doi:10.1038/s41586-026-10735-w")
DATAVERSE_DOI = "10.7910/DVN/7WTH1N"


def default_cache_dir() -> Path:
    env = os.environ.get("MIOBA_BANC_DATA")
    if env:
        return Path(env)
    return Path(__file__).resolve().parents[4] / "data" / "cache" / \
        "banc_888"


def source_url(name: str) -> str:
    return f"{BASE_URL}/{name}"


def cache_path(name: str, cache_dir: str | Path | None = None) -> Path:
    return Path(cache_dir or default_cache_dir()) / name
