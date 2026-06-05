#!/usr/bin/env bash
# Regenerate the TechDocs source symlinks in docs-techdocs/ from the canonical
# docs. This is what keeps Backstage TechDocs automatic: any MkDocs-safe page
# added to docs/ or any design spec added to plans/ is picked up on the next run
# (the TechDocs CI workflow runs this before every build).
#
# Authored pages in docs-techdocs/ (index.md, backstage.md) are left untouched;
# only the symlinks are managed. Jekyll-only / Liquid-templated pages
# (index.md, showcase.md, blog.md, _posts, _layouts, _includes) are skipped
# because MkDocs can't parse them.
set -euo pipefail

cd "$(dirname "$0")/.."

DEST="docs-techdocs"
DESIGN="$DEST/design"

# Pages in docs/ that are Jekyll-only or contain Liquid — never symlink these.
SKIP_DOCS=("index.md" "showcase.md" "blog.md")

is_skipped() {
  local name="$1"
  for s in "${SKIP_DOCS[@]}"; do [[ "$name" == "$s" ]] && return 0; done
  return 1
}

# Clear out previously generated symlinks (leave real authored files in place).
find "$DEST" -maxdepth 1 -type l -delete
rm -rf "$DESIGN"
mkdir -p "$DESIGN"

# Top-level canonical files → docs-techdocs/<name>
ln -s ../CONTRIBUTING.md "$DEST/contributing.md"
ln -s ../SECURITY.md "$DEST/security.md"

# MkDocs-safe pages from the Jekyll docs/ tree.
shopt -s nullglob
for f in docs/*.md; do
  name="$(basename "$f")"
  is_skipped "$name" && continue
  ln -s "../docs/$name" "$DEST/$name"
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
  ln -s "../../plans/$name" "$DESIGN/$name"
  title="$(grep -m1 '^# ' "$f" | sed 's/^#\s*//')"
  [[ -z "$title" ]] && title="$name"
  echo "- [$title]($name)" >>"$DESIGN/index.md"
done

echo "techdocs-sync: linked $(find "$DEST" -type l | wc -l | tr -d ' ') source files into $DEST/"
