//! Selection harness CLI: run genomes through one scenario, then compare.
//!
//! No model, network or credential is involved: every individual runs on
//! the fake Persona and fake reflector with a fixed clock and seeded IDs.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kamimusuhi_core::genome::Genome;
use kamimusuhi_runtime::RuntimeError;
use kamimusuhi_runtime::selection::{
    Candidate, HarnessConfig, RESULTS_FILE_NAME, Scenario, aggregate, compare_markdown,
    explorer_presets, population, read_results, run,
};

const USAGE: &str = "\
kamimusuhi-selection <command> [options]

commands:
  run --out <dir> [--scenario baseline|exploration|distractor] [--seed <n>]
      [--repeats <n>] [--mutants <n>] [--rate <0..1>] [--explorers]
      [--parent <genome.json>] [--genome <label>=<genome.json>]...
      [--reflection-interval <turns>] [--turns <n>] [--json]
                    run parent + mutants (+ explorer presets, + extra
                    genomes) through the scenario, --repeats seeds each;
                    writes <dir>/results.jsonl (one line per individual and
                    run) and one runtime directory per run; prints the
                    comparison table
  compare <results.jsonl | dir> [--json]
                    gene → phenotype → branch → cost → outcome table from a
                    results file (--json: per-individual aggregates)

scenarios: baseline (10 ordinary turns) | exploration (a fact stated in
words the later question shares nothing with; success = the fact reached the
workspace on the question turn, which only the exploration branch can do)
--explorers adds low-exploration (p=0.05) and high-exploration (p=0.65)
variants of the parent; the parent itself is the default (p=0.35).
defaults: --scenario baseline --seed 1 --repeats 1 --mutants 2 --rate 1.0
--reflection-interval 3. Run k of every individual uses ID seed seed+k, so
the same seed and genomes reproduce every metric; wall_ms is the one
column that is not.
";

fn main() -> ExitCode {
    match run_cli() {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(RuntimeError::Usage(message)) => {
            eprintln!("kamimusuhi-selection: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("kamimusuhi-selection: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Default)]
struct Options {
    positional: Vec<String>,
    out: Option<PathBuf>,
    seed: Option<u64>,
    mutants: Option<u32>,
    rate: Option<f64>,
    parent: Option<PathBuf>,
    genomes: Vec<(String, PathBuf)>,
    reflection_interval: Option<u32>,
    turns: Option<usize>,
    json: bool,
    scenario: Option<String>,
    repeats: Option<u32>,
    explorers: bool,
}

fn usage(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Usage(message.into())
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, RuntimeError> {
        let mut options = Self::default();
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| usage(format!("{arg} requires a value")))
            };
            match arg.as_str() {
                "--out" => options.out = Some(PathBuf::from(value()?)),
                "--seed" => options.seed = Some(parse_num(&value()?, "--seed")?),
                "--mutants" => options.mutants = Some(parse_num(&value()?, "--mutants")?),
                "--turns" => options.turns = Some(parse_num(&value()?, "--turns")?),
                "--reflection-interval" => {
                    options.reflection_interval =
                        Some(parse_num(&value()?, "--reflection-interval")?);
                }
                "--rate" => {
                    let rate: f64 = parse_num(&value()?, "--rate")?;
                    if !(0.0..=1.0).contains(&rate) {
                        return Err(usage("--rate must be within 0..1"));
                    }
                    options.rate = Some(rate);
                }
                "--parent" => options.parent = Some(PathBuf::from(value()?)),
                "--genome" => {
                    let raw = value()?;
                    let (label, path) = raw
                        .split_once('=')
                        .ok_or_else(|| usage("--genome expects <label>=<path>"))?;
                    if label.is_empty() || label.contains(['/', '\\']) {
                        return Err(usage("--genome label must be a plain name"));
                    }
                    options
                        .genomes
                        .push((label.to_owned(), PathBuf::from(path)));
                }
                "--json" => options.json = true,
                "--scenario" => options.scenario = Some(value()?),
                "--repeats" => options.repeats = Some(parse_num(&value()?, "--repeats")?),
                "--explorers" => options.explorers = true,
                other if other.starts_with("--") => {
                    return Err(usage(format!("unknown option {other}")));
                }
                _ => options.positional.push(arg),
            }
        }
        Ok(options)
    }
}

fn parse_num<T: std::str::FromStr>(raw: &str, flag: &str) -> Result<T, RuntimeError> {
    raw.parse()
        .map_err(|_| usage(format!("{flag} {raw:?} is not a valid number")))
}

fn load_genome(path: &Path) -> Result<Genome, RuntimeError> {
    let path = if path.is_dir() {
        path.join(Genome::FILE_NAME)
    } else {
        path.to_path_buf()
    };
    Ok(Genome::load(&path)?)
}

fn run_cli() -> Result<String, RuntimeError> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| usage("no command given"))?;
    let options = Options::parse(args)?;
    match command.as_str() {
        "run" => {
            let out = options
                .out
                .clone()
                .ok_or_else(|| usage("run requires --out <dir>"))?;
            let mut config = HarnessConfig::default();
            if let Some(name) = &options.scenario {
                config.scenario = Scenario::named(name)
                    .ok_or_else(|| usage(format!("unknown scenario {name:?}")))?;
            }
            if let Some(repeats) = options.repeats {
                if repeats == 0 {
                    return Err(usage("--repeats must be at least 1"));
                }
                config.repeats = repeats;
            }
            if let Some(seed) = options.seed {
                config.seed = seed;
            }
            if let Some(mutants) = options.mutants {
                config.mutants = mutants;
            }
            if let Some(rate) = options.rate {
                config.mutation_rate = rate;
            }
            if let Some(interval) = options.reflection_interval {
                config.reflection_interval_turns = interval;
            }
            if let Some(turns) = options.turns {
                if turns == 0 || turns > config.scenario.turns.len() {
                    return Err(usage(format!(
                        "--turns must be within 1..={}",
                        config.scenario.turns.len()
                    )));
                }
                config.scenario.truncate(turns);
            }
            let parent = match &options.parent {
                Some(path) => load_genome(path)?,
                None => Genome::with_id(format!("gen-sel-{}-parent", config.seed)),
            };
            let mut extra = Vec::new();
            if options.explorers {
                extra.extend(explorer_presets(&parent));
            }
            for (label, path) in &options.genomes {
                extra.push(Candidate {
                    label: label.clone(),
                    genome: load_genome(path)?,
                });
            }
            let candidates = population(&parent, &config, extra);
            let results = run(&out, &config, &candidates)?;
            let mut report = String::new();
            report.push_str(&format!(
                "results: {}\nscenario: {}  seed: {}  repeats: {}  individuals: {}  turns: {}  reflection_interval: {}\n\n",
                out.join(RESULTS_FILE_NAME).display(),
                config.scenario.name,
                config.seed,
                config.repeats,
                candidates.len(),
                config.scenario.turns.len(),
                config.reflection_interval_turns
            ));
            if options.json {
                report.push_str(
                    &serde_json::to_string_pretty(&aggregate(&results)).expect("serializes"),
                );
            } else {
                report.push_str(&compare_markdown(&results));
            }
            Ok(report)
        }
        "compare" => {
            let target = options
                .positional
                .first()
                .ok_or_else(|| usage("compare requires <results.jsonl | dir>"))?;
            let path = Path::new(target);
            let path = if path.is_dir() {
                path.join(RESULTS_FILE_NAME)
            } else {
                path.to_path_buf()
            };
            let results = read_results(&path)?;
            if results.is_empty() {
                return Err(usage(format!("{} holds no results", path.display())));
            }
            if options.json {
                Ok(serde_json::to_string_pretty(&aggregate(&results)).expect("serializes"))
            } else {
                Ok(compare_markdown(&results))
            }
        }
        "--help" | "-h" | "help" => Ok(USAGE.to_owned()),
        other => Err(usage(format!("unknown command {other:?}"))),
    }
}
