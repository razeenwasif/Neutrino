#!/bin/bash
# Move repositories from GitHub to the Neutrino server, one working copy at a time.
#
#   scripts/move-in.sh [--host mini] [--dry-run] [--disable-actions] DIR...
#
# For each DIR, a clone whose origin is on GitHub:
#   1. The server imports the GitHub repository under its GitHub name, every
#      branch and tag (`ntr import`), and from then on copies it back to GitHub
#      after every push, so GitHub keeps a full copy off the server. A
#      repository someone else owns is imported without that copy.
#   2. origin is pointed at the server. The GitHub URL stays as the `github`
#      remote, to look at. A push to it isn't lost, but it stops that
#      branch's copies until it's merged on the server.
#   3. origin is fetched, and branches with commits the server doesn't have
#      yet are listed, to push when you're ready.
# Another clone of a repository already on the server only gets steps 2 and 3.
#
# --disable-actions also turns GitHub Actions off for the repositories this
# imports, so the copies pushed to GitHub don't start workflows (or use
# minutes) there. It needs the gh CLI. Leave it off for a repository whose
# GitHub workflow you still want, such as a GitHub Pages deploy.
set -euo pipefail

host=mini
dry=0
disable_actions=0
dirs=()
while [ $# -gt 0 ]; do
    case "$1" in
        --host) host="$2"; shift 2 ;;
        --dry-run) dry=1; shift ;;
        --disable-actions) disable_actions=1; shift ;;
        -h|--help) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) echo "unknown option $1" >&2; exit 2 ;;
        *) dirs+=("$1"); shift ;;
    esac
done
[ ${#dirs[@]} -gt 0 ] || { echo "usage: scripts/move-in.sh [--host mini] [--dry-run] [--disable-actions] DIR..." >&2; exit 2; }

ssh_opts=(-o BatchMode=yes -o ConnectTimeout=15)
ntr=".neutrino/bin/ntr"

# Run ntr on the server, each argument quoted for its shell.
remote_ntr() {
    local cmd="$ntr" a
    for a in "$@"; do cmd+=" $(printf '%q' "$a")"; done
    ssh "${ssh_opts[@]}" "$host" "$cmd"
}

# owner and name from a GitHub URL, or nothing.
parse_github() {
    local url="$1" rest
    case "$url" in
        git@github.com:*) rest="${url#git@github.com:}" ;;
        ssh://git@github.com/*) rest="${url#ssh://git@github.com/}" ;;
        https://github.com/*) rest="${url#https://github.com/}" ;;
        *) return 1 ;;
    esac
    rest="${rest%/}"
    rest="${rest%.git}"
    case "$rest" in */*/*|"") return 1 ;; */*) ;; *) return 1 ;; esac
    printf '%s %s\n' "${rest%%/*}" "${rest#*/}"
}

me=""
if command -v gh >/dev/null 2>&1; then me=$(gh api user -q .login 2>/dev/null || true); fi

echo "Asking $host what it hosts…"
hosted=$(remote_ntr repos) || { echo "Can't reach ntr on $host." >&2; exit 1; }
# `ntr repos` prints: id  branch  path. Ids are lower-cased, so match on the
# path, which ends in <name>.git.
hosted_path() { printf '%s\n' "$hosted" | awk -v n="$1.git" '{ p = $NF; sub(/.*\//, "", p); if (p == n) { print $NF; exit } }'; }

failed=()
moved=()
for dir in "${dirs[@]}"; do
    echo
    if ! top=$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null); then
        echo "$dir: not a git working copy; skipped"; failed+=("$dir"); continue
    fi
    origin=$(git -C "$top" remote get-url origin 2>/dev/null || true)
    case "$origin" in
        ssh://"$host"/*) echo "$top: origin is already $host"; continue ;;
    esac
    if ! parsed=$(parse_github "$origin"); then
        echo "$top: origin ($origin) isn't a GitHub repository; skipped"; failed+=("$top"); continue
    fi
    read -r owner name <<<"$parsed"
    github="git@github.com:$owner/$name.git"
    echo "$top  ($owner/$name)"

    path=$(hosted_path "$name")
    if [ -z "$path" ]; then
        args=(import "$name" --from "$github")
        if [ -n "$me" ] && [ "$owner" != "$me" ]; then
            args+=(--no-mirror)
            echo "  $owner owns it, so it's imported without a copy back to GitHub"
        fi
        if [ -n "$me" ]; then
            desc=$(gh repo view "$owner/$name" --json description -q .description 2>/dev/null || true)
            [ -n "$desc" ] && args+=(-d "$desc")
        fi
        if [ $dry = 1 ]; then
            echo "  would run on $host: ntr ${args[*]}"
            path="/…/.neutrino/repos/$name.git"
        else
            echo "  importing on $host (every branch and tag)…"
            if ! remote_ntr "${args[@]}" | sed 's/^/  /'; then
                echo "  import failed; left as it was"; failed+=("$top"); continue
            fi
            hosted=$(remote_ntr repos)
            path=$(hosted_path "$name")
            [ -n "$path" ] || { echo "  imported, but $host doesn't list it; left as it was"; failed+=("$top"); continue; }
            if [ $disable_actions = 1 ] && [ "$owner" = "$me" ]; then
                if gh api -X PUT "repos/$owner/$name/actions/permissions" -F enabled=false >/dev/null; then
                    echo "  GitHub Actions turned off for $owner/$name"
                else
                    echo "  couldn't turn GitHub Actions off for $owner/$name"
                fi
            fi
        fi
    else
        echo "  already hosted at $path"
    fi

    url="ssh://$host$path"
    if [ $dry = 1 ]; then
        echo "  would set origin to $url and keep $origin as \`github\`"
        continue
    fi
    git -C "$top" remote get-url github >/dev/null 2>&1 || git -C "$top" remote add github "$origin"
    git -C "$top" remote set-url origin "$url"
    if git -C "$top" config --get remote.origin.pushurl >/dev/null; then
        git -C "$top" config --unset-all remote.origin.pushurl
    fi
    echo "  origin is now $url"
    if ! git -C "$top" fetch --quiet --prune origin; then
        echo "  fetching from $host failed; origin is set, fetch again later"; failed+=("$top"); continue
    fi
    # What's here that the server doesn't have yet.
    unpushed=()
    local_only=()
    while read -r branch upstream; do
        if [ -z "$upstream" ]; then
            local_only+=("$branch")
        elif ahead=$(git -C "$top" rev-list --count "$upstream..$branch" 2>/dev/null) && [ "$ahead" -gt 0 ]; then
            unpushed+=("$branch (+$ahead)")
        fi
    done < <(git -C "$top" for-each-ref --format='%(refname:short) %(upstream:short)' refs/heads)
    list() {
        local n=$# shown
        shown=$(printf '%s, ' "${@:1:6}"); shown=${shown%, }
        [ "$n" -gt 6 ] && shown+=" and $((n - 6)) more"
        printf '%s' "$shown"
    }
    [ ${#unpushed[@]} -gt 0 ] && echo "  commits to push: $(list "${unpushed[@]}")"
    [ ${#local_only[@]} -gt 0 ] && echo "  ${#local_only[@]} branch(es) only on this Mac: $(list "${local_only[@]}")"
    moved+=("$name")
done

echo
[ ${#moved[@]} -gt 0 ] && echo "Moved: ${moved[*]}"
if [ ${#failed[@]} -gt 0 ]; then
    echo "Not moved: ${failed[*]}"
    exit 1
fi
