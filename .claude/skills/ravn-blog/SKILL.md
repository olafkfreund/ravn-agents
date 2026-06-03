---
name: ravn-blog
description: >-
  Draft, preview, and publish a Ravn devlog post to the GitHub Pages site.
  Use for the /blog command or whenever the user wants to write, edit, or ship
  a blog/devlog entry for the ravn-agents project. Covers voice, front matter,
  file naming, what to write about, local preview, and the publish flow.
---

# Blogging for Ravn

The Ravn site is a Jekyll blog published via GitHub Pages from `docs/` on
`main`. This skill is the playbook for writing devlog posts that fit the
project — both **how** to publish and **what** is worth a post.

Live site: <https://olafkfreund.github.io/ravn-agents/> · Blog index: `/blog/`.

## What the blog is for

It's a **devlog**: progress, design decisions, and the dead ends (usually the
useful part). Ravn is built in the open, MIT-licensed. The blog is where we
narrate the build — milestone ships, architecture calls and their trade-offs,
NixOS/Rust specifics, deployment stories, and honest accounts of what didn't
work.

Hold the project's two anchors in mind; good posts usually reinforce one:

1. **The LLM is never in the detection hot path.** Deterministic tooling
   decides whether something is wrong and fires the alarm; the model only
   writes the explanation. Safe failure.
2. **Built in the open**, for other people's weird hardware, odd log formats,
   and hard-won operational scars.

Voice: plain language, technical but not stuffy, first-person plural ("we"),
concrete. Short paragraphs. No marketing fluff. It's fine — encouraged — to
write about struggles and reversals.

## Where posts live & how they're named

- Directory: `docs/_posts/`
- Filename: `YYYY-MM-DD-kebab-title.md` — **the date comes from the filename**,
  so it must be correct and in the past/today.
- One post per file. Drafts you don't want published yet go in
  `docs/_drafts/` (no date prefix) and are excluded from the build.

## Front matter (canonical schema)

```yaml
---
title: "M0 walks: an event from agent to Postgres"
description: "The first end-to-end thread is alive — what M0 means and how the pieces fit."
author: "Olaf"
tags: ["devlog", "m0", "rust", "nats"]
---
```

- `title` — required. Sentence case, specific, not clickbait.
- `description` — required. One or two sentences; shown in the post list and
  used by `jekyll-seo-tag` for meta/OG and by RSS. Make it stand alone.
- `author` — your name/handle.
- `tags` — lowercase. Conventional tags: `devlog`, the milestone (`m0`…`m5`),
  and topical (`rust`, `nixos`, `nats`, `postgres`, `inference`, `portal`,
  `decision`, `retro`).
- The post layout is applied automatically (a `defaults` rule sets
  `layout: post`), so you do **not** add `layout:`. Do not use `pubDate` — it
  is ignored by Jekyll; the filename date wins.

## Anatomy of a good Ravn post

1. **Hook** — one or two sentences on what happened and why it matters.
2. **Context** — where this sits (milestone/epic, what came before).
3. **What changed** — the concrete thing, with a little code or config when it
   earns its place (fenced blocks, keep them short).
4. **How / why** — the decision and the trade-off. This is the valuable part.
   Name the alternative you rejected and why.
5. **What's next** — the following step or open question. Invite input.

Keep it tight — most posts are 300–800 words. Wrap prose around ~90 columns to
match the existing posts and keep diffs readable.

### Linking conventions

- **Internal links** (other site pages) must be baseurl-aware — use Liquid:
  `[Roadmap]({{ '/roadmap/' | relative_url }})`. The site baseurl is
  `/ravn-agents`, so never hard-code `/roadmap/`.
- **GitHub links** (issues, PRs, discussions) use full URLs:
  `https://github.com/olafkfreund/ravn-agents/issues/27`.
- Reference the issue/epic or PR a post is about — it ties the narrative to the
  tracker.

### What not to publish

Secrets, tokens, internal infra hostnames, unsanitised logs, or details of an
unfixed security weakness. Sanitise any real log snippets (these also make great
`tests/fixtures/`).

## Topic playbook — what's worth a post

- **Milestone ships** (M0…M5): what the milestone means, the thread that now
  works end-to-end, a short demo.
- **Epic/feature lands**: e.g. detection taps, local inference, the portal —
  what it does and one design decision behind it.
- **Architecture decisions**: transport (NATS vs WebSocket), the partitioned
  events schema, the Event/Message contract, CPU-only inference choices. State
  the trade-off.
- **NixOS / Rust craft**: the flake + devenv setup, the hardened `services.ravn`
  modules, crane builds, the workspace — things other Nix/Rust folks learn from.
- **Model & eval**: tokens/sec on real CPUs, prompt regressions, why a model was
  chosen or dropped.
- **Deployment stories & retros**: running a fleet, what broke, what we'd do
  differently.

When unsure if something's post-worthy: did we make a non-obvious decision, hit
a surprising wall, or reach a visible milestone? If yes, write it.

## Workflow

### 1. Draft
Create `docs/_posts/YYYY-MM-DD-title.md` with the front matter above and the
five-part structure. Use today's date (the repo's current date) unless
back-dating is intended.

### 2. Preview locally
GitHub Pages will build it, but preview first to catch Liquid/front-matter
errors. Quick one-off build with Nix (no global Ruby needed):

```bash
mkdir -p /tmp/ravnblog && cat > /tmp/ravnblog/Gemfile <<'EOF'
source "https://rubygems.org"
gem "jekyll", "~> 4.3"
gem "jekyll-feed"
gem "jekyll-seo-tag"
EOF
nix shell nixpkgs#ruby nixpkgs#bundler nixpkgs#gcc nixpkgs#gnumake -c bash -c '
  export BUNDLE_GEMFILE=/tmp/ravnblog/Gemfile BUNDLE_PATH=/tmp/ravnblog/vendor
  bundle install
  bundle exec jekyll serve -s docs --port 4111   # http://localhost:4111/ravn-agents/
'
```

Check: the post renders wrapped (has the header + theme toggle), appears on
`/blog/` and on the home "Devlog" list, and the build prints no Liquid errors.
(Use a non-default port — the user runs services on default ports.)

### 3. Publish
The site builds from `docs/` on `main`, so a post is live once it's on `main`.
Default to a PR:

```bash
git checkout -b blog/YYYY-MM-DD-title
git add docs/_posts/YYYY-MM-DD-title.md
git commit -m "Blog: <title>"
git push -u origin HEAD
gh pr create --base main --title "Blog: <title>" --body "..."
```

After merge, optionally trigger/await the Pages build and verify:

```bash
gh api -X POST repos/olafkfreund/ravn-agents/pages/builds --jq .status
# poll repos/olafkfreund/ravn-agents/pages/builds/latest -> "built"
curl -s -o /dev/null -w "%{http_code}\n" https://olafkfreund.github.io/ravn-agents/blog/
```

**Always confirm with the user before committing/pushing/opening a PR** — it's
outward-facing.

## Pre-publish checklist

- [ ] Filename `YYYY-MM-DD-title.md`, date correct
- [ ] `title` + `description` present; no `layout:`/`pubDate`
- [ ] Internal links use `{{ '...' | relative_url }}`; GitHub links are full URLs
- [ ] Linked the relevant issue/epic/PR
- [ ] Reinforces an anchor (last-mile LLM / in-the-open) where natural
- [ ] No secrets or unsanitised logs
- [ ] Previews cleanly (renders on `/blog/` and home, no Liquid errors)
- [ ] Tags include `devlog` + milestone + topical

## Theme note

The site uses an in-repo Gruvbox theme with a light/dark toggle (CSS variables;
`docs/assets/css/main.css`). Posts are plain Markdown — styling is automatic.
Comments are giscus (GitHub Discussions), live once configured in `_config.yml`.
