#!/usr/bin/env bash
# Regenerate the TechDocs source files in docs-techdocs/ from the canonical docs.
#
# These are REAL COPIES, not symlinks: Backstage's TechDocs reader downloads the
# repo via its GitHub UrlReader, whose archive extraction does not reliably
# preserve symlinks — committed symlinks make the built-in builder fail with a
# "no docs directory" error. Copies are read identically everywhere.
#
# Freshness is enforced by the TechDocs CI workflow's drift check: if running
# this script would change anything, the build fails with "run techdocs-sync".
# So a doc added to docs/ or plans/ must be synced + committed, and it then
# flows into Backstage automatically.
#
# Authored pages (index.md, backstage.md) are preserved; everything else under
# docs-techdocs/ is generated. Jekyll-only / Liquid pages (index.md, showcase.md,
# blog.md) are skipped because MkDocs can't parse them.
set -euo pipefail

cd "$(dirname "$0")/.."

DEST="docs-techdocs"
DESIGN="$DEST/design"

# Pages in docs/ that are Jekyll-only or contain Liquid — never copy these.
SKIP_DOCS=("index.md" "showcase.md" "blog.md")

is_skipped() {
  local name="$1"
  for s in "${SKIP_DOCS[@]}"; do [[ "$name" == "$s" ]] && return 0; done
  return 1
}

# Remove previously generated copies AND any legacy symlinks (keep the authored
# pages and this dir).
find "$DEST" -maxdepth 1 \( -type f -o -type l \) ! -name index.md ! -name backstage.md -delete
rm -rf "$DESIGN"
mkdir -p "$DESIGN"

# Top-level canonical files → docs-techdocs/<name>
cp CONTRIBUTING.md "$DEST/contributing.md"
cp SECURITY.md "$DEST/security.md"
# The self-contained demo guide (incl. GPU/Ollama setup) → docs-techdocs/demo.md
cp demo/README.md "$DEST/demo.md"

# MkDocs-safe pages from the Jekyll docs/ tree.
shopt -s nullglob
for f in docs/*.md; do
  name="$(basename "$f")"
  is_skipped "$name" && continue
  cp "$f" "$DEST/$name"
done

# Every design spec under plans/ → docs-techdocs/design/<name>, plus a generated
# index listing them (so the Design section has a landing page and new specs
# show up there automatically).
{
  echo "# Design specs"
  echo
  echo "In-repo design documents, synced from \`plans/\`."
  echo
} >"$DESIGN/index.md"
for f in plans/*.md; do
  name="$(basename "$f")"
  cp "$f" "$DESIGN/$name"
  title="$(grep -m1 '^# ' "$f" | sed 's/^#\s*//')"
  [[ -z "$title" ]] && title="$name"
  echo "- [$title]($name)" >>"$DESIGN/index.md"
done

echo "techdocs-sync: copied $(find "$DEST" -type f ! -name index.md ! -name backstage.md | wc -l | tr -d ' ') generated files into $DEST/"
