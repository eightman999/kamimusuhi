"""Fi3 — Runtime Identity Guard.

Detects unintended changes to the components that constitute the
Kamimusuhi runtime (model, tokenizer, prompts, adapter, sampling,
versions, dependency lock) and refuses silent continuation on BREAKING
drift.  See experiments/fi3/README.md.
"""
