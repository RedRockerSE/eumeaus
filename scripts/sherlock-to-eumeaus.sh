#!/usr/bin/env bash
# Runs Sherlock (https://github.com/sherlock-project/sherlock — a much
# larger site catalog than the bundled eumeaus-username-search-plugin,
# at the cost of not being a signed, isolated Eumeaus plugin) against a
# username and merges every claimed account it finds into an existing
# Eumeaus case: one OnlineAccount entity per site, linked to the case's
# existing Username entity by a HasAccount relationship — the same
# entity/relationship shape eumeaus-username-search-plugin itself uses.
#
# Requires:
#   - eumeaus (the CLI) on PATH — see README.md's Installation section,
#     or set EUMEAUS_BIN to an explicit path.
#   - sherlock on PATH — `pip install sherlock-project`, or set
#     SHERLOCK_BIN to an explicit path.
#   - python3 — used to parse Sherlock's --csv output and the case's
#     own `case export --format report` JSON. No third-party Python
#     packages needed.
#
# Usage:
#   scripts/sherlock-to-eumeaus.sh --case <path/to/case.eum> --username <name> [-- <extra sherlock args>]
#
# The Username entity must already exist in the case — this script
# doesn't create it, matching `scan run`'s own "target must already
# exist" convention:
#   eumeaus --case <case> entity add --type Username --key <name>
#
# Anything after a literal `--` is passed straight through to sherlock
# (e.g. `-- --site GitHub --site GitLab --timeout 30`).
#
# Idempotent: safe to re run against the same case/username later to
# pick up newly-claimed sites. `entity add` already auto-merges on an
# exact (entity_type, canonical_key) match, but plain `relationship
# add` does *not* dedupe identical (from, to, type) triples — this
# script checks the case's own exported report first and skips an
# OnlineAccount entity or HasAccount relationship a previous run
# already created, rather than re-adding it (which would just spam
# duplicate attribute facts/relationship rows every re-run).

set -euo pipefail

usage() {
  cat <<'EOF'
Usage: sherlock-to-eumeaus.sh --case <path/to/case.eum> --username <name> [-- <extra sherlock args>]

Environment overrides:
  EUMEAUS_BIN    path to the eumeaus CLI binary (default: eumeaus on PATH)
  SHERLOCK_BIN   path to the sherlock binary (default: sherlock on PATH)
EOF
}

CASE=""
USERNAME=""
EXTRA_SHERLOCK_ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --case)
      CASE="$2"
      shift 2
      ;;
    --username)
      USERNAME="$2"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    --)
      shift
      EXTRA_SHERLOCK_ARGS=("$@")
      break
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "$CASE" ] || [ -z "$USERNAME" ]; then
  echo "error: --case and --username are required" >&2
  usage >&2
  exit 2
fi

EUMEAUS_BIN="${EUMEAUS_BIN:-eumeaus}"
SHERLOCK_BIN="${SHERLOCK_BIN:-sherlock}"

command -v "$EUMEAUS_BIN" >/dev/null 2>&1 || {
  echo "error: '$EUMEAUS_BIN' not found. Install it — see README.md's Installation section — or set EUMEAUS_BIN." >&2
  exit 1
}
command -v "$SHERLOCK_BIN" >/dev/null 2>&1 || {
  echo "error: '$SHERLOCK_BIN' not found. Install it: pip install sherlock-project (or set SHERLOCK_BIN)." >&2
  exit 1
}
command -v python3 >/dev/null 2>&1 || {
  echo "error: python3 is required (to parse Sherlock's CSV and the case's report export) but was not found" >&2
  exit 1
}

if [ ! -f "$CASE" ]; then
  echo "error: case file not found: $CASE" >&2
  exit 1
fi

# Find the existing Username entity for $USERNAME. entity list prints
# tab-separated `id  type  canonical_key  display_label`; canonical_key
# is already trimmed+lowercased by the engine, so normalize the same
# way here rather than requiring an exact-case match.
NORMALIZED_USERNAME="$(printf '%s' "$USERNAME" | tr '[:upper:]' '[:lower:]' | sed 's/^ *//; s/ *$//')"
USERNAME_ENTITY_ID="$("$EUMEAUS_BIN" --case "$CASE" entity list --type Username |
  awk -F'\t' -v key="$NORMALIZED_USERNAME" '$3 == key { print $1; exit }')"

if [ -z "$USERNAME_ENTITY_ID" ]; then
  echo "error: no Username entity with key '$USERNAME' in this case yet." >&2
  echo "Add it first: $EUMEAUS_BIN --case \"$CASE\" entity add --type Username --key \"$USERNAME\"" >&2
  exit 1
fi

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

echo "Running Sherlock against '$USERNAME'..." >&2
set +e
"$SHERLOCK_BIN" --csv --folderoutput "$WORKDIR" "${EXTRA_SHERLOCK_ARGS[@]}" "$USERNAME"
SHERLOCK_EXIT=$?
set -e

# Sherlock exits 1 whenever the username was claimed on zero sites in
# that run — not a real failure, just nothing new to import. Only a
# missing CSV (a real crash, or an unrecognized extra argument) is
# treated as fatal here.
CSV_FILE="$WORKDIR/$USERNAME.csv"
if [ ! -f "$CSV_FILE" ]; then
  echo "error: Sherlock did not produce $CSV_FILE (exit code $SHERLOCK_EXIT) — see its output above" >&2
  exit 1
fi

REPORT_FILE="$WORKDIR/report.json"
"$EUMEAUS_BIN" case export "$CASE" --out "$REPORT_FILE" --format report

# Cross-references Sherlock's CSV against the case's current state and
# prints one plan line per claimed site:
#   site<TAB>url<TAB>canonical_key<TAB>existing_entity_id_or_empty<TAB>needs_relationship(0|1)
# All the "does this already exist" lookups happen here, once, instead
# of one `entity add`/relationship check per site.
PLAN_FILE="$WORKDIR/plan.tsv"
python3 - "$CSV_FILE" "$REPORT_FILE" "$USERNAME_ENTITY_ID" "$USERNAME" >"$PLAN_FILE" <<'PY'
import csv
import json
import sys

csv_path, report_path, username_entity_id, username = sys.argv[1:5]

with open(report_path) as f:
    report = json.load(f)

entities_by_key = {
    (e["entity_type"], e["canonical_key"]): e["id"] for e in report["entities"]
}
existing_relationships = {
    (r["from_entity_id"], r["to_entity_id"], r["relationship_type"])
    for r in report["relationships"]
}

seen_sites = set()
with open(csv_path, newline="") as f:
    for row in csv.DictReader(f):
        if row.get("exists") != "Claimed":
            continue
        site = row["name"]
        if site in seen_sites:
            continue  # Sherlock shouldn't repeat a site, but never trust that blindly.
        seen_sites.add(site)

        url = row.get("url_user") or row.get("url_main") or ""
        canonical_key = f"{site}:{username}".lower()
        existing_id = entities_by_key.get(("OnlineAccount", canonical_key), "")
        needs_relationship = "1"
        if existing_id and (username_entity_id, existing_id, "HasAccount") in existing_relationships:
            needs_relationship = "0"

        # Pipe-delimited, not tab: bash's `read` treats a tab as "IFS
        # whitespace" and silently collapses a run of them, so an empty
        # field (a blank existing_id, the common case) between two
        # tabs gets swallowed instead of preserved — misaligning every
        # column after it. A non-whitespace delimiter doesn't have
        # that quirk. Confirmed live: an earlier tab-delimited version
        # of this script silently misread every plan line's last two
        # columns on a fresh case (no existing entities to skip) as
        # "already present", skipping every relationship it should
        # have added.
        print(f"{site}|{url}|{canonical_key}|{existing_id}|{needs_relationship}")
PY

ADDED=0
SKIPPED=0

while IFS='|' read -r SITE URL CANONICAL_KEY EXISTING_ID NEEDS_RELATIONSHIP; do
  [ -z "$SITE" ] && continue

  if [ -n "$EXISTING_ID" ]; then
    ACCOUNT_ID="$EXISTING_ID"
  else
    ACCOUNT_ID="$("$EUMEAUS_BIN" --case "$CASE" entity add --type OnlineAccount \
      --key "$SITE:$USERNAME" --attr "site=$SITE" --attr "url=$URL")"
  fi

  if [ "$NEEDS_RELATIONSHIP" = "1" ]; then
    "$EUMEAUS_BIN" --case "$CASE" relationship add \
      --from "$USERNAME_ENTITY_ID" --to "$ACCOUNT_ID" --type HasAccount >/dev/null
    ADDED=$((ADDED + 1))
    echo "  + $SITE: $URL" >&2
  else
    SKIPPED=$((SKIPPED + 1))
  fi
done <"$PLAN_FILE"

echo "Done: $ADDED new HasAccount relationship(s) added, $SKIPPED already present." >&2
