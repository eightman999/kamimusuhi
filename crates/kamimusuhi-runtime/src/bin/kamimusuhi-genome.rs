//! Genome CLI: inspect, create, clone, mutate and diff genome files.
//!
//! This tool never opens the canonical store and never reads memory. It
//! works on `genome.json` files only, so it can run against a live runtime
//! directory without claiming a writer epoch. The runtime picks up a changed
//! file on its next start.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kamimusuhi_core::genome::{Genome, GenomeError, diff};

const USAGE: &str = "\
kamimusuhi-genome <command> [options]

commands:
  new <path> [--id <name>]              write a fresh default genome
  inspect <path>                        genome, genes and phenotype as JSON
  phenotype <path>                      derived runtime settings only
  clone <src> <dst> [--id <name>]       same genes, new instance
  mutate <src> <dst> --seed <n> [--rate <0..1>] [--id <name>]
                                        deterministic mutation (rate default 0.2)
  set <path> <gene> <value>             set one gene in place (clamped to 0..1)
  diff <a> <b>                          changed genes and distance as JSON

<path> may be a genome.json or a runtime directory containing one.
Genes are named group.name, e.g. temperament.curiosity. --id is optional;
without it a random gen-<hex> ID is minted.
";

fn main() -> ExitCode {
    match run() {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(Failure::Usage(message)) => {
            eprintln!("kamimusuhi-genome: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Genome(error)) => {
            eprintln!("kamimusuhi-genome: {error}");
            ExitCode::FAILURE
        }
    }
}

enum Failure {
    Usage(String),
    Genome(GenomeError),
}

impl From<GenomeError> for Failure {
    fn from(error: GenomeError) -> Self {
        Self::Genome(error)
    }
}

#[derive(Default)]
struct Options {
    positional: Vec<String>,
    id: Option<String>,
    seed: Option<u64>,
    rate: Option<f64>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, Failure> {
        let mut options = Self::default();
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| Failure::Usage(format!("{arg} requires a value")))
            };
            match arg.as_str() {
                "--id" => options.id = Some(value()?),
                "--seed" => {
                    let raw = value()?;
                    options.seed = Some(
                        raw.parse()
                            .map_err(|_| Failure::Usage(format!("--seed {raw:?} is not a u64")))?,
                    );
                }
                "--rate" => {
                    let raw = value()?;
                    let rate: f64 = raw
                        .parse()
                        .map_err(|_| Failure::Usage(format!("--rate {raw:?} is not a number")))?;
                    if !(0.0..=1.0).contains(&rate) {
                        return Err(Failure::Usage("--rate must be within 0..1".to_owned()));
                    }
                    options.rate = Some(rate);
                }
                other if other.starts_with("--") => {
                    return Err(Failure::Usage(format!("unknown option {other}")));
                }
                _ => options.positional.push(arg),
            }
        }
        Ok(options)
    }

    fn positional(&self, index: usize, name: &str) -> Result<&str, Failure> {
        self.positional
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| Failure::Usage(format!("missing <{name}>")))
    }
}

/// A runtime directory resolves to its genome file; anything else is taken
/// as the file itself.
fn resolve(path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_dir() {
        path.join(Genome::FILE_NAME)
    } else {
        path.to_path_buf()
    }
}

fn load(path: &str) -> Result<Genome, Failure> {
    Ok(Genome::load(&resolve(path))?)
}

fn save(genome: &Genome, path: &str) -> Result<(), Failure> {
    Ok(genome.save(&resolve(path))?)
}

fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).expect("report serializes")
}

fn run() -> Result<String, Failure> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or_else(|| Failure::Usage("no command given".to_owned()))?;
    let options = Options::parse(args)?;
    match command.as_str() {
        "new" => {
            let path = options.positional(0, "path")?;
            let genome = match &options.id {
                Some(id) => Genome::with_id(id.clone()),
                None => Genome::new_default(),
            };
            save(&genome, path)?;
            Ok(json(&serde_json::json!({
                "written": resolve(path).display().to_string(),
                "genome_id": genome.genome_id,
                "hash": genome.hash,
            })))
        }
        "inspect" => {
            let genome = load(options.positional(0, "path")?)?;
            Ok(json(&serde_json::json!({
                "genome": genome,
                "genes": genome.genes(),
                "phenotype": genome.phenotype(),
            })))
        }
        "phenotype" => {
            let genome = load(options.positional(0, "path")?)?;
            Ok(json(&genome.phenotype()))
        }
        "clone" => {
            let src = load(options.positional(0, "src")?)?;
            let dst = options.positional(1, "dst")?;
            let child = match &options.id {
                Some(id) => src.clone_genome_with_id(id.clone()),
                None => src.clone_genome(),
            };
            save(&child, dst)?;
            Ok(json(&serde_json::json!({
                "written": resolve(dst).display().to_string(),
                "genome_id": child.genome_id,
                "parent_id": child.parent_id,
                "version": child.version,
                "hash": child.hash,
            })))
        }
        "mutate" => {
            let src = load(options.positional(0, "src")?)?;
            let dst = options.positional(1, "dst")?;
            let seed = options
                .seed
                .ok_or_else(|| Failure::Usage("mutate requires --seed <n>".to_owned()))?;
            let rate = options.rate.unwrap_or(0.2);
            let child = match &options.id {
                Some(id) => src.mutate_with_id(seed, rate, id.clone()),
                None => src.mutate(seed, rate),
            };
            save(&child, dst)?;
            Ok(json(&serde_json::json!({
                "written": resolve(dst).display().to_string(),
                "genome_id": child.genome_id,
                "parent_id": child.parent_id,
                "version": child.version,
                "hash": child.hash,
                "seed": seed,
                "rate": rate,
                "diff": diff(&src, &child),
            })))
        }
        "set" => {
            let path = options.positional(0, "path")?;
            let key = options.positional(1, "gene")?;
            let raw = options.positional(2, "value")?;
            let value: f64 = raw
                .parse()
                .map_err(|_| Failure::Usage(format!("value {raw:?} is not a number")))?;
            let mut genome = load(path)?;
            let before = genome.gene(key)?;
            genome.set_gene(key, value)?;
            save(&genome, path)?;
            Ok(json(&serde_json::json!({
                "written": resolve(path).display().to_string(),
                "gene": key,
                "before": before,
                "after": genome.gene(key)?,
                "hash": genome.hash,
            })))
        }
        "diff" => {
            let a = load(options.positional(0, "a")?)?;
            let b = load(options.positional(1, "b")?)?;
            Ok(json(&diff(&a, &b)))
        }
        "--help" | "-h" | "help" => Ok(USAGE.to_owned()),
        other => Err(Failure::Usage(format!("unknown command {other:?}"))),
    }
}
