---
description: Draft, preview, and publish a Ravn devlog post
argument-hint: "[topic or post idea, e.g. \"M0 walking skeleton is done\"]"
---

Write a devlog post for the Ravn project blog (GitHub Pages, `docs/_posts/`).

**Use the `ravn-blog` skill** — it holds the full playbook: voice, front-matter
schema, file naming, what's worth a post, local preview, and the publish flow.
Follow it.

Topic / idea from the user: **$ARGUMENTS**

Process:
1. If `$ARGUMENTS` is empty or vague, propose 2–3 post ideas drawn from recent
   work (merged PRs, closed issues, milestone progress) and ask which to write.
2. Draft the post per the skill (`docs/_posts/YYYY-MM-DD-title.md`, today's date,
   correct front matter, the five-part structure, baseurl-aware internal links,
   linked issues/PRs).
3. Preview locally with the skill's Jekyll build and confirm it renders cleanly
   on `/blog/` and the home Devlog list with no Liquid errors.
4. Show the user the draft. **Get explicit approval before committing, pushing,
   or opening a PR** — publishing is outward-facing.
5. On approval, publish via a `blog/<date>-<title>` branch + PR to `main`
   (Pages builds from `main`); after merge, verify the live URL.
