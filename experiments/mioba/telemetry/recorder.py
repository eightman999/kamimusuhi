"""Telemetry recorder: writes SensorEvents into the lineage DB and mirrors
a JSONL stream under <runs>/<exp>/telemetry/ for offline inspection."""
from __future__ import annotations

import json
from pathlib import Path


class TelemetryRecorder:
    def __init__(self, db, experiment_id: str, telemetry_dir: str | Path | None = None):
        self.db = db
        self.experiment_id = experiment_id
        self._stream = None
        if telemetry_dir:
            p = Path(telemetry_dir)
            p.mkdir(parents=True, exist_ok=True)
            self._stream = (p / "samples.jsonl").open("a", encoding="utf-8",
                                                      buffering=1)

    def record(self, samples: list[dict]) -> int:
        if not samples:
            return 0
        if self._stream:
            for s in samples:
                self._stream.write(json.dumps(s, default=str) + "\n")
        return self.db.insert_telemetry(self.experiment_id, samples)

    def close(self):
        if self._stream:
            self._stream.close()
            self._stream = None
