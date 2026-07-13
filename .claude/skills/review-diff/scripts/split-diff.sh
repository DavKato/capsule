#!/usr/bin/env bash
set -euo pipefail

# Usage: split-diff.sh <pr-number|diff-source> "group1:file1,file2" "group2:file3,file4" ...
#        some-command | split-diff.sh - "group1:file1,file2" ...
# Fetches the diff (or reads stdin when source is "-"), splits by group, prints temp file paths.

diff_source="$1"
shift

if [[ $# -eq 0 ]]; then
  echo "Error: no groups provided" >&2
  exit 1
fi

if [[ "$diff_source" == "-" ]]; then
  full_diff=$(cat)
else
  full_diff=$(gh pr diff "$diff_source" 2>/dev/null) || \
  full_diff=$(git diff "$diff_source" 2>/dev/null) || \
  { echo "Error: could not fetch diff for '$diff_source'" >&2; exit 1; }
fi

safe_source="${diff_source//\//-}"
tmpdir=$(mktemp -d "/tmp/review-${safe_source}-XXXXXX")

map_lines=()
for group_arg in "$@"; do
  group_name="${group_arg%%:*}"
  file_list="${group_arg#*:}"
  IFS=',' read -ra files <<< "$file_list"
  for f in "${files[@]}"; do
    map_lines+=("${f}"$'\t'"${group_name}")
  done
done

export FILE_GROUP_MAP
FILE_GROUP_MAP=$(printf '%s\n' "${map_lines[@]}")

printf '%s\n' "$full_diff" | awk -v tmpdir="$tmpdir" '
BEGIN {
  n = split(ENVIRON["FILE_GROUP_MAP"], pairs, "\n")
  for (i = 1; i <= n; i++) {
    split(pairs[i], kv, "\t")
    file2group[kv[1]] = kv[2]
  }
  outfile = ""
}
/^diff --git / {
  bpath = ""
  for (i = 1; i <= NF; i++) {
    if (substr($i, 1, 2) == "b/") {
      bpath = substr($i, 3)
    }
  }
  if (bpath in file2group) {
    outfile = tmpdir "/" file2group[bpath] ".diff"
  } else {
    outfile = ""
  }
}
outfile != "" { print >> outfile }
'

for group_arg in "$@"; do
  group_name="${group_arg%%:*}"
  outfile="${tmpdir}/${group_name}.diff"
  if [[ -f "$outfile" ]]; then
    echo "$outfile"
  fi
done
