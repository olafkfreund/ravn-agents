//! `ravn-eval` — explanation-quality benchmarks for small local models (#157).
//!
//! The core claim Ravn rests on is that a sub-2B CPU model is good enough for
//! the explanation last-mile. This harness is the proof: it scores candidate
//! models on a fixed corpus of real-shaped events for **faithfulness** (no
//! invented facts), **actionability** (a concrete, targeted check), and
//! **latency/RAM** on CPU — then emits a comparison table and a publishable
//! site page.
//!
//! Scoring is deterministic and model-free (no LLM-as-judge), so a run is
//! reproducible and `nix flake check`-able. The model itself is behind a
//! pluggable backend:
//!
//! ```text
//! ravn-eval                                  # reproducible: replay recorded runs, score, emit table + site page
//! ravn-eval --endpoint http://127.0.0.1:8080 --models qwen3-1.7b,qwen2.5-3b   # live llama-server benchmark
//! ravn-eval --validate                       # offline: just load + validate the corpus
//! ```

mod backend;
mod fixtures;
mod report;
mod rubric;
mod runner;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

use backend::{LlamaServerBackend, RecordedBackend};

struct Args {
    endpoint: Option<String>,
    models: Vec<String>,
    corpus: PathBuf,
    out: Option<PathBuf>,
    validate_only: bool,
}

fn parse_args() -> Args {
    let mut endpoint = None;
    let mut models = Vec::new();
    let mut corpus = fixtures::default_corpus_dir();
    let mut out = None;
    let mut validate_only = false;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--endpoint" => endpoint = it.next(),
            "--models" | "--model" => {
                if let Some(v) = it.next() {
                    models.extend(
                        v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                    );
                }
            }
            "--corpus" => {
                if let Some(v) = it.next() {
                    corpus = PathBuf::from(v);
                }
            }
            "--out" => out = it.next().map(PathBuf::from),
            "--validate" => validate_only = true,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }
    Args { endpoint, models, corpus, out, validate_only }
}

fn print_usage() {
    eprintln!(
        "ravn-eval — explanation-quality benchmarks for small local models (#157)\n\n\
         USAGE:\n  \
         ravn-eval [--endpoint URL --models a,b,c] [--corpus DIR] [--out FILE] [--validate]\n\n\
         Default (no flags): replays the committed recorded runs through the live\n\
         scoring harness and prints a comparison table — fully reproducible, no\n\
         model required. Use --out to also write a markdown site page.\n\n\
         With --endpoint: benchmarks each --models entry against a running\n\
         llama-server, measuring real latency and RAM.\n\n\
         With --validate: loads and validates the corpus, then exits."
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args();
    let corpus = fixtures::load_corpus(&args.corpus)
        .with_context(|| format!("loading corpus from {}", args.corpus.display()))?;
    println!("loaded {} fixtures from {}", corpus.len(), args.corpus.display());

    if args.validate_only {
        for f in &corpus {
            println!("  {:<28} [{}]", f.name, f.reference.category.label());
        }
        println!("\ncorpus OK; {} fixtures validated.", corpus.len());
        return Ok(());
    }

    let rows = if let Some(endpoint) = args.endpoint {
        // Live mode: benchmark each requested model against the server.
        let endpoint = endpoint.trim_end_matches('/').to_string();
        let models = if args.models.is_empty() { vec![String::new()] } else { args.models };
        let mut rows = Vec::new();
        for model in &models {
            let label = if model.is_empty() { "<server default>" } else { model };
            println!("\nbenchmarking {label} @ {endpoint} (live)");
            let backend = LlamaServerBackend::new(
                label.to_string(),
                model.clone(),
                endpoint.clone(),
                Duration::from_secs(120),
            )?;
            match runner::bench_live(&backend, &corpus).await {
                Ok(row) => rows.push(row),
                Err(error) => eprintln!("  model '{label}' failed: {error:#}"),
            }
        }
        rows
    } else {
        // Default mode: replay the committed recorded runs. This is what makes
        // the harness reproducible in CI and as a published page.
        let root = RecordedBackend::recordings_dir();
        println!("\nscoring committed recorded runs from {} (recorded)", root.display());
        runner::run_all_recorded(&root, &corpus)
            .await
            .with_context(|| format!("replaying recorded runs from {}", root.display()))?
    };

    if rows.is_empty() {
        anyhow::bail!("no models produced results");
    }

    println!("\n## Model comparison\n");
    print!("{}", report::render_table(&rows));
    println!("\n## By category\n");
    print!("{}", report::render_category_table(&rows));

    if let Some(out) = args.out {
        let page = report::render_site_page(&rows, &corpus);
        std::fs::write(&out, page).with_context(|| format!("writing site page {}", out.display()))?;
        println!("\nwrote site page to {}", out.display());
    }

    Ok(())
}
