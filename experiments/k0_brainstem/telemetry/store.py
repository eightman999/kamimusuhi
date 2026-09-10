"""Append-only JSONL metrics with a shared, concurrent-reader WAL index."""
import json
import re
import sqlite3
import time
from pathlib import Path

RUN_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")


class MetricStore:
    def __init__(self, artifacts, run_id):
        if not RUN_ID.fullmatch(run_id):
            raise ValueError("invalid run_id")
        self.run_id = run_id
        self.root = Path(artifacts)
        run = self.root / "runs" / run_id
        run.mkdir(parents=True, exist_ok=True)
        self.stream = (run / "metrics.jsonl").open("a", encoding="utf-8", buffering=1)
        self.db = sqlite3.connect(self.root / "metrics.sqlite", timeout=30)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA busy_timeout=30000")
        self.db.execute("CREATE TABLE IF NOT EXISTS metrics (id INTEGER PRIMARY KEY, run_id TEXT NOT NULL, timestamp REAL NOT NULL, payload TEXT NOT NULL)")
        self.db.execute("CREATE INDEX IF NOT EXISTS metrics_run ON metrics(run_id,id)")
        self.db.commit()

    def write(self, metric):
        record = dict(metric)
        record.setdefault("timestamp", time.time())
        record["run_id"] = self.run_id
        payload = json.dumps(record, allow_nan=False, separators=(",", ":"))
        self.stream.write(payload + "\n")
        self.db.execute("INSERT INTO metrics(run_id,timestamp,payload) VALUES(?,?,?)", (self.run_id, record["timestamp"], payload))
        self.db.commit()

    def close(self):
        self.stream.close()
        self.db.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
