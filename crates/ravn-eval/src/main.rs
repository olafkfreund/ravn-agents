//! `ravn-eval` — model eval / benchmark harness (#38).
//!
//! Benchmarks candidate models on the #39 fixture set: prompt-processing and
//! generation throughput, end-to-end latency, server memory, and a
//! deterministic quality score — emitting a markdown comparison table to guide
//! the default-model choice.
//!
//! ```text
//! ravn-eval --endpoint http://127.0.0.1:8080 --models qwen2.5-3b,phi-3.5-mini
//! ravn-eval                       # offline: validate fixtures, no model needed
//! ```

mod report;
mod rubric;
mod runner;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

struct Args {
    endpoint: Option<String>,
    models: Vec<String>,
    fixtures: PathBuf,
}

fn parse_args() -> Args {
    let mut endpoint = None;
    let mut models = Vec::new();
    let mut fixtures = runner::default_fixtures_dir();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--endpoint" => endpoint = it.next(),
            "--models" | "--model" => {
                if let Some(v) = it.next() {
                    models.extend(v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
                }
            }
            "--fixtures" => {
                if let Some(v) = it.next() {
                    fixtures = PathBuf::from(v);
                }
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }
    Args { endpoint, models, fixtures }
}

fn print_usage() {
    eprintln!(
        "ravn-eval — benchmark models on the fixture set (#38)\n\n\
         USAGE:\n  \
         ravn-eval [--endpoint URL] [--models a,b,c] [--fixtures DIR]\n\n\
         With --endpoint, benchmarks each model against a running llama-server\n\
         and prints a comparison table. Without it, validates the fixtures\n\
         offline (no model required)."
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args();
    let fixtures = runner::load_events(&args.fixtures)?;
    println!("loaded {} fixtures from {}", fixtures.len(), args.fixtures.display());

    let Some(endpoint) = args.endpoint else {
        // Offline mode: prove the fixtures load and render. Useful in CI and as
        // a smoke check before spinning up a model.
        if let Some((name, event)) = fixtures.first() {
            println!("\nsample prompt for '{name}':\n---");
            print!("{}", ravn_agent::inference::build_user_prompt(event));
            println!("---");
        }
        println!("\nno --endpoint given; skipping live benchmark.");
        return Ok(());
    };
    let endpoint = endpoint.trim_end_matches('/').to_string();

    let models = if args.models.is_empty() { vec![String::new()] } else { args.models };
    let http = reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?;

    let mut rows = Vec::new();
    for model in &models {
        let label = if model.is_empty() { "<server default>" } else { model };
        println!("\nbenchmarking {label} @ {endpoint}");
        match runner::bench_model(&http, &endpoint, model, &fixtures).await {
            Ok(row) => rows.push(row),
            Err(error) => eprintln!("  model '{label}' failed: {error:#}"),
        }
    }

    if rows.is_empty() {
        anyhow::bail!("no models produced results");
    }

    println!("\n## Model comparison\n");
    print!("{}", report::render_table(&rows));
    Ok(())
}
