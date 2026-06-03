# Prompt-regression fixtures (#39)

Golden fixtures that pin the **deterministic** parts of inference so a prompt or
parsing change shows up as a reviewable diff — no live model required. The
`fixture_*` tests in `src/inference.rs` discover these files automatically, so
adding a case is just dropping in data files (no Rust to write).

## `prompts/` — event → rendered user prompt

Each case is a pair sharing a base name:

- `<name>.event.json` — a serialized `ravn_core::Event` (a sanitised, realistic
  detection event).
- `<name>.prompt.txt` — the exact user prompt `build_user_prompt` produces for
  that event.

The test renders every `*.event.json` and asserts the output equals the
matching `*.prompt.txt`.

**Add a case:** write the `.event.json`, then generate the golden prompt:

```sh
RAVN_BLESS=1 cargo test -p ravn-agent fixture_prompts
```

Review the generated `.prompt.txt` in your diff before committing.

## `explanations/` — model response → parsed explanation

Locks `parse_explanation` against messy real-world model output (JSON wrapped in
prose, missing fields, non-JSON fallback). Each case is a pair:

- `<name>.response.txt` — raw `content` as a model might return it.
- `<name>.expected.json` — `{ "explanation": string, "suggested_check": string|null }`.

The test parses every `*.response.txt` and asserts the result matches.

## Blessing

`RAVN_BLESS=1` regenerates `*.prompt.txt` from the current `build_user_prompt`.
It never rewrites `explanations/` expectations — those are hand-authored on
purpose so a parsing regression can't silently re-bless itself.
