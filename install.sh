#!/bin/sh
set -eu

die() { printf '%s\n' "$*" >&2; exit 1; }

prefix="${PREFIX:-$HOME/.local}"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix)
      [ "$#" -ge 2 ] && [ -n "$2" ] || die "--prefix requires a directory"
      prefix="$2"
      shift
      ;;
    -h|--help)
      cat <<'HELP'
Usage: install.sh [--prefix DIR]
Installs Yerux in DIR/bin (default: ~/.local/bin).
Run from an unpacked release or pipe the script to sh to download a release.
YERUX_VERSION selects a stable release or release candidate (e.g. 1.0.0-rc1).
The default is the latest stable release.
HELP
      exit 0
      ;;
    *) die "unknown option: $1" ;;
  esac
  shift
done

# ---- look: colors and a spinner only on a real terminal (NO_COLOR / dumb TERM disable them) ----
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
  fancy=1
  esc="$(printf '\033')"
  b="${esc}[1m"; g="${esc}[32m"; y="${esc}[33m"; d="${esc}[2m"; z="${esc}[0m"
  v="${esc}[38;5;135m"; bl="${esc}[38;5;33m"; c="${esc}[36m"; r="${esc}[31m"
  ok='✓'; warn='!'
else
  fancy=0
  b=; g=; y=; d=; z=; v=; bl=; c=; r=
  ok='ok'; warn='warning:'
fi

# run "label" command args...: spinner while it runs, then a check mark (or a cross and its status)
run() {
  label="$1"; shift
  if [ "$fancy" = 0 ]; then
    printf '  %s ...\n' "$label"
    "$@"
    return
  fi
  "$@" &
  pid=$!
  i=0
  while kill -0 "$pid" 2>/dev/null; do
    case $((i % 10)) in
      0) f='⠋' ;; 1) f='⠙' ;; 2) f='⠹' ;; 3) f='⠸' ;; 4) f='⠼' ;;
      5) f='⠴' ;; 6) f='⠦' ;; 7) f='⠧' ;; 8) f='⠇' ;; *) f='⠏' ;;
    esac
    if [ $((i % 20)) -lt 10 ]; then col="$v"; else col="$bl"; fi
    printf '\r  %s%s%s %s' "$col" "$f" "$z" "$label"
    i=$((i + 1))
    sleep 0.08
  done
  if wait "$pid"; then
    printf '\r\033[K  %s%s%s %s\n' "$g" "$ok" "$z" "$label"
  else
    printf '\r\033[K  %s✗%s %s\n' "$r" "$z" "$label"
    return 1
  fi
}

if [ "$fancy" = 1 ]; then
  printf '\n  %s╲ ╱%s  %s%sYerux%s %sinstaller%s\n' "$v" "$z" "$b" "$bl" "$z" "$d" "$z"
  printf '  %s ╳ %s  %sownership, borrow checking, native code%s\n\n' "$bl" "$z" "$d" "$z"
fi

tmp=
candidate=
cleanup() {
  if [ -n "$candidate" ]; then rm -f -- "$candidate"; fi
  if [ -n "$tmp" ]; then rm -rf -- "$tmp"; fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

here=
case "$0" in
  install.sh|*/install.sh)
    if [ -f "$0" ]; then here="$(dirname -- "$0")"; fi
    ;;
esac
if [ -n "$here" ] && [ -x "$here/bin/yerux" ] &&
   [ -z "${YERUX_URL:-}${YERUX_VERSION:-}${YERUX_RELEASE_BASE_URL:-}" ]; then
  src="$here/bin/yerux"
  version=
else
  [ "$(uname -s)" = Linux ] || die "release downloads require Linux"
  case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    aarch64|arm64) arch=aarch64 ;;
    *) die "unsupported CPU architecture: $(uname -m)" ;;
  esac
  base="${YERUX_RELEASE_BASE_URL:-https://github.com/yervix10/yerux/releases}"
  base="${base%/}"
  url="${YERUX_URL:-}"
  version="${YERUX_VERSION:-}"
  if [ -n "$url" ] && [ -z "$version" ]; then
    archive="${url##*/}"
    version="${archive#yerux-}"
    version="${version%-linux-$arch.tar.gz}"
  elif [ -z "$version" ]; then
    version="$(curl -fsSL "$base/latest/download/VERSION")"
  fi
  version="${version#v}"
  case "$version" in
    ''|*[!0-9.rc-]*) die "invalid release version: $version" ;;
  esac
  printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc[1-9][0-9]*)?$' ||
    die "invalid release version: $version"

  name="yerux-$version-linux-$arch"
  archive="$name.tar.gz"
  url="${url:-$base/download/v$version/$archive}"
  tmp="$(mktemp -d)"
  hash="${YERUX_SHA256:-}"
  if [ -z "$hash" ]; then
    curl -fsSL "$url.sha256" -o "$tmp/checksum"
    hash="$(awk -v file="$archive" '
      NF == 2 && ($2 == file || $2 == "*" file) { hash = $1; count++ }
      END { if (count != 1) exit 1; print hash }
    ' "$tmp/checksum")" || die "missing or ambiguous checksum for $archive"
  fi
  [ "${#hash}" -eq 64 ] || die "invalid SHA-256"
  case "$hash" in *[!0-9a-fA-F]*) die "invalid SHA-256" ;; esac

  run "Downloading yerux $version ($arch)" curl -fsSL "$url" -o "$tmp/$archive" ||
    die "download failed: $url"
  run "Verifying SHA-256" sh -c 'cd "$1" && printf "%s  %s\n" "$2" "$3" | sha256sum -c - >/dev/null' _ "$tmp" "$hash" "$archive" ||
    die "checksum mismatch for $archive"
  src="$tmp/yerux"
  run "Unpacking" sh -c 'tar -xOzf "$1" -- "$2" > "$3"' _ "$tmp/$archive" "$name/bin/yerux" "$src" ||
    die "could not unpack $archive"
fi

mkdir -p -- "$prefix/bin"
candidate="$(mktemp "$prefix/bin/.yerux.XXXXXX")"
install -m 755 -- "$src" "$candidate"
reported="$("$candidate" --version)"
if [ -n "$version" ] && [ "$reported" != "yerux $version" ]; then
  die "binary reports '$reported', expected 'yerux $version'"
fi
mv -fT -- "$candidate" "$prefix/bin/yerux"
candidate=

# ---- summary ----
size=
if bytes="$(wc -c < "$prefix/bin/yerux" 2>/dev/null)" && [ -n "$bytes" ]; then
  size=" ($(( (bytes + 524288) / 1048576 )) MB)"
fi
printf '%s%s%s installed %s to %s/bin/yerux%s\n' "$g" "$ok" "$z" "$reported" "$prefix" "$size"

case ":$PATH:" in
  *":$prefix/bin:"*) on_path=1 ;;
  *) on_path=0 ;;
esac
if [ "$on_path" = 0 ]; then
  shown="$prefix/bin"
  case "$shown" in "${HOME:-/nonexistent}"/*) shown="\$HOME/${shown#"$HOME"/}" ;; esac
  printf '\n%s%s%s %s is not in your PATH. Add it for your shell:\n' "$y" "$warn" "$z" "$prefix/bin"
  sh_name="${SHELL:-}"; sh_name="${sh_name##*/}"
  case "$sh_name" in
    zsh)  printf '    echo '"'"'export PATH="%s:$PATH"'"'"' >> ~/.zshrc && exec zsh\n' "$shown" ;;
    fish) printf '    fish_add_path %s\n' "$shown" ;;
    bash) printf '    echo '"'"'export PATH="%s:$PATH"'"'"' >> ~/.bashrc && exec bash\n' "$shown" ;;
    *)    printf '    export PATH="%s:$PATH"   %s(add it to your shell profile)%s\n' "$shown" "$d" "$z" ;;
  esac
fi

if ! command -v cc >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1; then
  printf '\n%s%s%s no C compiler found: the default backend needs one (install gcc or clang);\n  %s--native%s programs build and run without it.\n' "$y" "$warn" "$z" "$b" "$z"
fi

printf '\n%sNext steps%s\n' "$b" "$z"
printf '    %syerux doctor%s                       %scheck what this installation can do%s\n' "$c" "$z" "$d" "$z"
printf '    %syerux new hello && cd hello%s        %sa project: yerux.toml + src/main.yx%s\n' "$c" "$z" "$d" "$z"
printf '    %syerux run%s                          %sbuild and run it%s\n' "$c" "$z" "$d" "$z"
printf '  Guide: %shttps://github.com/yervix10/yerux/blob/main/docs/GUIDE.md%s\n' "$bl" "$z"
printf '  Bugs:  %shttps://github.com/yervix10/yerux/issues%s\n' "$bl" "$z"
