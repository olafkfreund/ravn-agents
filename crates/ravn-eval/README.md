# ravn-eval — explanation-quality benchmarks for small local models (#157)

Ravn's detection is **deterministic** — taps fire the alarm, independent of any
LLM. The model only writes the plain-language explanation and suggests one
check. The open question is whether a **sub-2B CPU model is good enough for that
last mile**. This crate is the proof: it scores candidate models on a fixed
corpus of real-shaped events for **faithfulness**, **actionability**, and
**latency/RAM** on CPU, then emits a comparison table and a publishable site
page.

Scoring is **deterministic and model-free** (no LLM-as-judge), so a run is
reproducible and gates `nix flake check`. The model itself sits behind a
pluggable backend, and the crate ships **recorded runs** so the full scored
table is produced with zero model dependencies.

## Corpus structure

`fixtures/corpus/` — one pair per case, sharing a base name:

- `<name>.event.json` — a serialized `ravn_core::Event` (a sanitised but
  realistic detection event). These reuse the production event types, so the
  harness benchmarks the exact prompt the agent would send.
- `<name>.reference.json` — the gold-standard `Reference`:
  - `category` — one of `failed_unit`, `oom_kill`, `auth_anomaly`,
    `config_drift` (the four buckets #157 names).
  - `reference_explanation` — the human-authored explanation, shown in the
    report for qualitative comparison.
  - `must_mention` — salient facts a faithful answer must reference (grounding
    ground-truth, unioned with the payload-derived salient tokens).
  - `forbidden` — **hallucination traps**: causes *not* in the event. Naming one
    costs faithfulness points. This is how "no invented facts" is measured.
  - `expected_check` / `check_keywords` — the ideal check and the tokens an
    acceptable `suggested_check` should contain (drives actionability).

The shipped corpus has 6 cases spanning all four categories (failed units, host
OOM kill, Kubernetes OOMKilled, SSH brute-force, sshd config drift, and a
CrashLoopBackOff workload).

`fixtures/recordings/<model>/` — a recorded model run:

- `manifest.json` — `model`, `captured_on` (CPU/quant/server provenance), and
  the captured `latency_ms` / `gen_tps` / `prompt_tps` / `mem_mb` profile.
- `<name>.txt` — the raw model response for each corpus fixture, replayed and
  scored exactly as a live response would be.

Four models are recorded: `qwen3-1.7b`, `qwen2.5-3b`, `phi-3.5-mini`, and a
deliberately weak `tinyllama-1.1b` baseline that proves the rubric
discriminates.

## Scoring

Each component is in `0.0..=1.0`:

- **faithfulness** = `0.5·grounded + 0.5·no_hallucination` — grounded in the
  event's salient facts *and* free of invented facts.
- **actionability** = `0.5 + 0.5·check_specificity` when a check is present,
  else `0`. A generic "please investigate" scores ~0.5; a targeted command
  scores higher.
- **overall** = `0.6·faithfulness + 0.3·actionability + 0.1·length_ok`.

Faithfulness dominates: for the explanation last-mile, being right matters more
than being actionable, and far more than being verbose.

## How to run

```sh
# Reproducible default: replay the committed recorded runs, score, print the
# comparison table + per-category rollup. No model required.
cargo run -p ravn-eval

# Also write the publishable markdown site page.
cargo run -p ravn-eval -- --out crates/ravn-eval/RESULTS.md

# Just load + validate the corpus (CI smoke check).
cargo run -p ravn-eval -- --validate

# Live benchmark against a running llama-server (real latency + RAM):
cargo run -p ravn-eval -- --endpoint http://127.0.0.1:8080 --models qwen3-1.7b,qwen2.5-3b
```

`RESULTS.md` is a committed golden: the `results_page_matches_golden` test
re-renders it from the recordings and fails if it drifts. Re-bless after an
intended change:

```sh
RAVN_BLESS=1 cargo test -p ravn-eval results_page_matches_golden
```

## Mocked vs live

- **Mocked / recorded (shipped, reproducible):** the model *responses* and their
  latency/RAM profile in `fixtures/recordings/`. These were captured once and
  committed so the scored table is reproducible in CI and `nix flake check`
  without a GPU or even a model file. Every recorded row is labelled `recorded`
  in the table, and the site page carries a prominent note.
- **Live:** with `--endpoint`, the `LlamaServerBackend` calls a real
  `llama-server`, measures wall-clock latency and scrapes RSS from
  `/metrics`. The **scoring is identical** in both modes — only the source of
  the model text differs.
