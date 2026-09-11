# Papers

Kamimusuhi is a research program; papers should be **claim-centered slices** of it rather than attempts to describe the entire project at once.

No manuscript is implied to be accepted, peer reviewed, or even submitted merely because it exists here. Put its status at the top of its README/manuscript.

## Recommended layout

```text
papers/
├── README.md
└── PNNN-short-name/
    ├── README.md          # claim, status, linked experiments
    ├── manuscript/        # TeX/Markdown/source
    ├── figures/           # publication figures
    ├── results/           # compact tables/statistics used by the paper
    ├── repro/             # commands/configs needed to reproduce claims
    └── LICENSE            # optional paper-specific license if required
```

A paper directory should identify:

- one primary research question;
- the claim being tested;
- linked experiment reports and exact commits/tags;
- data/model provenance;
- baseline and ablation coverage;
- statistical procedure;
- known limitations;
- publication/preprint identifiers when available.

## Status vocabulary

Suggested manuscript statuses: `in-preparation`, `preprint`, `submitted`, `accepted`, `published`, `superseded`.

Do not rewrite an old result to match a later interpretation. Preserve the original experiment report and make the manuscript point to it.

## Citation and archival releases

The repository-level [`CITATION.cff`](../CITATION.cff) describes Kamimusuhi software. Once a paper has a DOI or other canonical citation, add a `preferred-citation` or paper-specific citation metadata as appropriate.

For reproducibility, freeze the code/config/data-manifest state used by a paper as a Git tag or release rather than citing a moving branch.

## Licensing

Publication artifacts may need a license different from repository code. A paper-local license notice takes precedence for that paper artifact. Third-party figures/data must retain their upstream terms and attribution. See [`../LICENSING.md`](../LICENSING.md).
