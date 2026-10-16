#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
# Install the repository's git hooks into this clone.
#
# pre-push runs tools/public-hygiene over the files and over the messages of the
# commits being pushed, and refuses the push when it finds anything. The rules are in
# .public-hygiene.json at the repository root.
set -eu
root=$(git rev-parse --show-toplevel)
hooks=$(git rev-parse --git-path hooks)
mkdir -p "$hooks"
cat > "$hooks/pre-push" <<'HOOK'
#!/bin/sh
# Installed by tools/install-hooks.sh.
set -eu
root=$(git rev-parse --show-toplevel)
zero=0000000000000000000000000000000000000000
python3 "$root/tools/public-hygiene" "$root"
while read -r _local_ref local_sha _remote_ref remote_sha; do
    [ "$local_sha" = "$zero" ] && continue          # a deletion: nothing to scan
    if [ "$remote_sha" = "$zero" ]; then
        range="$local_sha --not --remotes"          # a new branch: its commits not yet pushed anywhere
    else
        range="$remote_sha..$local_sha"
    fi
    python3 "$root/tools/public-hygiene" --commits "$range" "$root"
done
HOOK
chmod +x "$hooks/pre-push"
echo "installed $hooks/pre-push"
