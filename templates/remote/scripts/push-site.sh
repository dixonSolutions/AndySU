#!/bin/sh
# Commit and push the site directory to gh-pages (no-op when nothing changed).
set -eu
cd site
touch .nojekyll
git add -A
git diff --cached --quiet && { echo "site unchanged"; exit 0; }
git -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
  commit -q -m "$1"
git push -q origin HEAD:gh-pages
