//! Generational experiment CLI: founders → evaluate → survivors → offspring.
//!
//! Fully local: fake Persona, fake reflector, fixed clock, seeded IDs.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kamimusuhi_runtime::RuntimeError;
use kamimusuhi_runtime::evolution::{
    Environment, EvolutionConfig, read_experiment, report_markdown, run,
};
use kamimusuhi_runtime::evolution_batch::{self, BatchConfig};

const USAGE: &str = "\
kamimusuhi-evolution <command> [options]

commands:
  run --out <dir> [--env exploration|distractor|baseline] [--seed <n>]
      [--population <n>] [--generations <n>] [--repeats <n>] [--rate <0..1>]
      [--validation-seed <n>] [--validation-panel <n>]
      [--final-test-seed <n>] [--final-test-panel <n>]
                    seed random founders (from --seed and --population only,
                    so every environment starts from the same population),
                    then per generation: evaluate on the selection panel
                    (--repeats seeds, same for every individual, changing
                    per generation), apply the survival rule, evaluate on the
                    validation panel (--validation-panel seeds from --validation-seed, the
                    same every generation, never used for selection), carry
                    a seeded-shuffled half of the survivors over and fill the
                    rest by clone+mutate of the survivors.
                    Writes lineage.jsonl (per individual per generation, both
                    panels), generations.jsonl (selection side),
                    adaptation.jsonl (validation side), gen-<g>/ and validation-<g>/.
  report <dir>      selection table, validation table, gen 0 vs final, lineage
  batch --out <dir> [--replicates <n>] [--seed-base <n>] [--population <n>]
      [--generations <n>] [--repeats <n>] [--rate <0..1>]
      [--validation-seed <n>] [--validation-panel <n>]
      [--final-test-seed <n>] [--final-test-panel <n>] [--threads <n>]
                    run <replicates> independent populations (top-level seed
                    seed-base+r), each through all three environments from
                    the same founders; per replicate store gen 0 → final
                    deltas per environment and Δ(env) − Δ(baseline); write
                    replicates.jsonl, batch_summary.json (mean / median /
                    variance / seeded bootstrap 95% CI / sign counts) and
                    rep-<r>/<env>/ experiments. Replicates run in parallel;
                    the thread count changes no number.
  batch-report <dir>
                    per-replicate table and the statistics tables

environments (survival rule = completed all selection runs + rule below):
  exploration   fact recoverable only by exploring; success_rate >= 0.25
  distractor    fact found by baseline; exploring adds context and can
                displace the hit; success_rate >= 0.75, mean
                context_records <= 5
  baseline      no probe, no budget: completion only (drift)
A generation where nobody satisfied the rule is logged with
selection_failed=true and bred from the whole population.
defaults: --env exploration --seed 1 --population 8 --generations 5
--repeats 4 --rate 0.5 --validation-seed 900000 --validation-panel 16. No crossover, no
scalar fitness. Same arguments reproduce every logged number except wall_ms.
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
            eprintln!("kamimusuhi-evolution: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("kamimusuhi-evolution: {error}");
            ExitCode::FAILURE
        }
    }
}

fn usage(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Usage(message.into())
}

fn parse_num<T: std::str::FromStr>(raw: &str, flag: &str) -> Result<T, RuntimeError> {
    raw.parse()
        .map_err(|_| usage(format!("{flag} {raw:?} is not a valid number")))
}

#[derive(Default)]
struct Options {
    positional: Vec<String>,
    out: Option<PathBuf>,
    env: Option<String>,
    seed: Option<u64>,
    population: Option<usize>,
    generations: Option<u32>,
    repeats: Option<u32>,
    rate: Option<f64>,
    validation_seed: Option<u64>,
    validation_panel: Option<u32>,
    final_test_seed: Option<u64>,
    final_test_panel: Option<u32>,
    replicates: Option<u32>,
    seed_base: Option<u64>,
    threads: Option<usize>,
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
                "--env" => options.env = Some(value()?),
                "--seed" => options.seed = Some(parse_num(&value()?, "--seed")?),
                "--population" => options.population = Some(parse_num(&value()?, "--population")?),
                "--generations" => {
                    options.generations = Some(parse_num(&value()?, "--generations")?);
                }
                "--repeats" => options.repeats = Some(parse_num(&value()?, "--repeats")?),
                "--validation-seed" => {
                    options.validation_seed = Some(parse_num(&value()?, "--validation-seed")?)
                }
                "--validation-panel" => {
                    options.validation_panel = Some(parse_num(&value()?, "--validation-panel")?);
                }
                "--replicates" => options.replicates = Some(parse_num(&value()?, "--replicates")?),
                "--seed-base" => options.seed_base = Some(parse_num(&value()?, "--seed-base")?),
                "--threads" => options.threads = Some(parse_num(&value()?, "--threads")?),
                "--final-test-seed" => {
                    options.final_test_seed = Some(parse_num(&value()?, "--final-test-seed")?);
                }
                "--final-test-panel" => {
                    options.final_test_panel = Some(parse_num(&value()?, "--final-test-panel")?);
                }
                "--rate" => {
                    let rate: f64 = parse_num(&value()?, "--rate")?;
                    if !(0.0..=1.0).contains(&rate) {
                        return Err(usage("--rate must be within 0..1"));
                    }
                    options.rate = Some(rate);
                }
                other if other.starts_with("--") => {
                    return Err(usage(format!("unknown option {other}")));
                }
                _ => options.positional.push(arg),
            }
        }
        Ok(options)
    }
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
            let mut config = EvolutionConfig::default();
            if let Some(name) = &options.env {
                config.environment = Environment::named(name)
                    .ok_or_else(|| usage(format!("unknown environment {name:?}")))?;
            }
            if let Some(seed) = options.seed {
                config.seed = seed;
            }
            if let Some(n) = options.population {
                config.population_size = n;
            }
            if let Some(n) = options.generations {
                config.generations = n;
            }
            if let Some(n) = options.repeats {
                config.repeats = n;
            }
            if let Some(rate) = options.rate {
                config.mutation_rate = rate;
            }
            if let Some(seed) = options.validation_seed {
                config.validation_seed = seed;
            }
            if let Some(panel) = options.validation_panel {
                config.validation_panel = panel;
            }
            if let Some(seed) = options.final_test_seed {
                config.final_test_seed = seed;
            }
            if let Some(panel) = options.final_test_panel {
                config.final_test_panel = panel;
            }
            let experiment = run(&out, &config)?;
            Ok(format!(
                "out: {}\nenvironment: {}  seed: {}  population: {}  generations: {}  selection repeats: {}  mutation_rate: {}  validation panel: {} seeds from {}\n\n{}",
                out.display(),
                config.environment.name,
                config.seed,
                config.population_size,
                config.generations,
                config.repeats,
                config.mutation_rate,
                config.validation_panel,
                config.validation_seed,
                report_markdown(&experiment)
            ))
        }
        "batch" => {
            let out = options
                .out
                .clone()
                .ok_or_else(|| usage("batch requires --out <dir>"))?;
            let mut config = BatchConfig::default();
            if let Some(n) = options.replicates {
                config.replicates = n;
            }
            if let Some(n) = options.seed_base {
                config.seed_base = n;
            }
            if let Some(n) = options.population {
                config.population_size = n;
            }
            if let Some(n) = options.generations {
                config.generations = n;
            }
            if let Some(n) = options.repeats {
                config.repeats = n;
            }
            if let Some(rate) = options.rate {
                config.mutation_rate = rate;
            }
            if let Some(seed) = options.validation_seed {
                config.validation_seed = seed;
            }
            if let Some(panel) = options.validation_panel {
                config.validation_panel = panel;
            }
            if let Some(seed) = options.final_test_seed {
                config.final_test_seed = seed;
            }
            if let Some(panel) = options.final_test_panel {
                config.final_test_panel = panel;
            }
            if let Some(n) = options.threads {
                config.threads = n;
            }
            let batch = evolution_batch::run(&out, &config)?;
            Ok(format!(
                "out: {}\nreplicates: {}  seed_base: {}  population: {}  generations: {}  selection repeats: {}  mutation_rate: {}  validation panel: {} from {}  final-test panel: {} from {}\n\n{}",
                out.display(),
                config.replicates,
                config.seed_base,
                config.population_size,
                config.generations,
                config.repeats,
                config.mutation_rate,
                config.validation_panel,
                config.validation_seed,
                config.final_test_panel,
                config.final_test_seed,
                evolution_batch::report_markdown(&batch)
            ))
        }
        "batch-report" => {
            let dir = options
                .positional
                .first()
                .map(Path::new)
                .ok_or_else(|| usage("batch-report requires <dir>"))?;
            Ok(evolution_batch::report_markdown(
                &evolution_batch::read_batch(dir)?,
            ))
        }
        "report" => {
            let dir = options
                .positional
                .first()
                .map(Path::new)
                .ok_or_else(|| usage("report requires <dir>"))?;
            Ok(report_markdown(&read_experiment(dir)?))
        }
        "--help" | "-h" | "help" => Ok(USAGE.to_owned()),
        other => Err(usage(format!("unknown command {other:?}"))),
    }
}
