# Research data

This directory is the boundary for datasets used by Kamimusuhi research. It is **not** a blanket MIT-licensed data directory.

## Default policy

Prefer committing:

- download/query scripts;
- stable upstream identifiers;
- schema descriptions;
- checksums;
- small provenance metadata;
- transformation code;
- compact derived measurements whose redistribution is permitted.

Do not commit large raw datasets, private data, restricted/pre-publication data, or third-party data whose terms do not permit redistribution.

Suggested local layout:

```text
data/
├── README.md              # tracked policy
├── manifests/             # tracked provenance manifests
├── derived/               # only redistributable derived artifacts
├── raw/                   # local-only; gitignored
├── vendor/                # local-only third-party payloads; gitignored
├── private/               # local-only; gitignored
└── cache/                 # local-only; gitignored
```

## Provenance manifest

Every committed third-party-derived dataset should include a colocated manifest containing at least:

```yaml
name: example-dataset
upstream_source: https://example.org/
upstream_version: "snapshot-or-version"
retrieved_at: YYYY-MM-DD
upstream_license: SPDX-or-URL
redistribution: allowed | restricted | unknown
required_citation:
  - citation or DOI
transform:
  - script/path-or-description
source_checksums:
  - sha256:...
```

`redistribution: unknown` means the underlying payload should not be committed until clarified.

## FlyWire

FlyWire public-release data is upstream material and is not covered by Kamimusuhi's MIT license. Follow [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) and FlyWire's current public-release/citation guidance. Prefer reproducible extraction scripts and snapshot identifiers over vendoring the raw connectome.
