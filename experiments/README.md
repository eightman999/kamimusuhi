# Experiment disposition and production admission

Current policy: [2026-11-08 audit](../docs/research/experiment-dispositions-2026-11-08.md).
Machine-readable decisions: [experiment-dispositions.json](../knowledge/experiment-dispositions.json).

No learned organ is currently approved for Active production use. H0/R0 are
runtime evaluation candidates; all five existing organ descriptors are Shadow.
Synthetic PASS remains research evidence, not a production utility claim.

Historical reports/checkpoints and negative results are retained unchanged.
G0-v6 revision 1 is invalidated; its STRONG_PASS is not a current result.
MIOBA/A3/BODY0 findings are separate research, not a deployed resident roadmap.
Native Rust FI2/FI3 production verification is recorded separately from the
Python FI experiment results.

Check coverage, evidence hashes and admission agreement:

```sh
cargo test -p kamimusuhi-runtime --test experiment_dispositions
```
