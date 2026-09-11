# Third-party notices

Kamimusuhi builds on published research, open-source software, public datasets, and external services. Those resources are not automatically covered by Kamimusuhi's MIT license.

## FlyWire / Drosophila connectome

Kamimusuhi research notes may discuss or experimentally use FlyWire connectome data.

As of 2026-09-11, FlyWire states that its **public release data is available under CC BY-NC 4.0**. Use of FlyWire data must follow FlyWire's current licensing and citation guidance, including attribution to the applicable FlyWire publications and contributors.

- License identifier: `CC-BY-NC-4.0`
- FlyWire citation and public-release guidance: https://home.flywire.ai/guidelines
- FlyWire terms: https://flywire.ai/tos
- FlyWire / Codex resource: https://codex.flywire.ai/

The Kamimusuhi MIT license does not convert FlyWire data to MIT. Unless a future research artifact explicitly documents redistribution rights, this repository should contain **code, identifiers, provenance, metadata, and derived measurements where permitted**, rather than a bundled copy of the upstream connectome dataset.

Do not commit FlyWire pre-publication/restricted data unless its applicable access and redistribution terms explicitly permit doing so.

## Software dependencies

Rust/Python/system dependencies retain their respective upstream licenses. `Cargo.lock`, package manifests, adapters, or references to a dependency do not relicense that dependency.

## Models and hosted APIs

Model weights, tokenizer files, hosted model outputs, and API-provided content remain subject to the corresponding provider/model licenses and terms. A Kamimusuhi adapter being MIT-licensed does not imply that the connected model or service is MIT-licensed.

When a new third-party dataset or substantial redistributed artifact is added, add its provenance and license here or in a colocated notice before merging it.
