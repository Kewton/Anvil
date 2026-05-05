#!/usr/bin/env bash
# Smoke-test the release install script without network access.

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SCRIPT="$REPO_ROOT/scripts/install.sh"

for cmd in gzip shasum bash; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "SKIP: $cmd not installed" >&2
    exit 0
  fi
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

source_dir="$tmp/source"
fake_bin="$tmp/bin"
install_dir="$tmp/install"
mkdir -p "$source_dir" "$fake_bin" "$install_dir"

printf 'fake-anvil\n' > "$source_dir/anvil"
gzip -c "$source_dir/anvil" > "$source_dir/anvil-linux-amd64.gz"
(cd "$source_dir" && shasum -a 256 anvil-linux-amd64.gz > anvil-linux-amd64.gz.sha256)

cat > "$fake_bin/uname" <<'SH'
#!/usr/bin/env bash
case "$1" in
  -s) printf 'Linux\n' ;;
  -m) printf 'x86_64\n' ;;
  *) exec /usr/bin/uname "$@" ;;
esac
SH
chmod +x "$fake_bin/uname"

cat > "$fake_bin/curl" <<SH
#!/usr/bin/env bash
set -euo pipefail
url=
out=
while [[ \$# -gt 0 ]]; do
  case "\$1" in
    -o)
      out=\$2
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      url=\$1
      shift
      ;;
  esac
done
if [[ -z "\$url" || -z "\$out" ]]; then
  echo "curl fixture expected URL and -o output" >&2
  exit 2
fi
cp "$source_dir/\$(basename "\$url")" "\$out"
SH
chmod +x "$fake_bin/curl"

PATH="$fake_bin:$PATH" ANVIL_INSTALL_DIR="$install_dir" bash "$SCRIPT" >/dev/null

test -x "$install_dir/anvil"
test "$(cat "$install_dir/anvil")" = "fake-anvil"

echo "PASS: install script smoke test"
