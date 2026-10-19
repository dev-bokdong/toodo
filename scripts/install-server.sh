#!/usr/bin/env bash
# Builds toodo-server from this checkout and installs it for the current user:
#   ~/.local/bin/toodo-server                     the server binary
#   ~/.local/share/toodo-server-web               the web build it serves
#   ~/.config/systemd/user/toodo-server.service   a systemd user service (unless --no-service)
# Safe to run again: an update is `git pull` followed by this script.
# See docs/server.md.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/install-server.sh [--listen ADDR] [--no-service]

  --listen ADDR   address the service listens on (default 127.0.0.1:47616)
  --no-service    build and install only; do not write or restart the systemd service
EOF
}

listen=127.0.0.1:47616
service=1
while [ $# -gt 0 ]; do
  case "$1" in
    --listen)
      [ $# -ge 2 ] || { usage >&2; exit 2; }
      listen=$2
      shift 2
      ;;
    --listen=*)
      listen=${1#--listen=}
      shift
      ;;
    --no-service)
      service=0
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

log() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

if ! [[ $listen =~ ^(.+):([0-9]+)$ ]]; then
  die "--listen must be HOST:PORT, got '$listen'"
fi
listen_host=${BASH_REMATCH[1]}
listen_port=${BASH_REMATCH[2]}
if [ "$listen_port" = 47615 ]; then
  die "port 47615 is the desktop app's MCP tunnel port; pick another (see docs/server.md)"
fi
# Health checks reach a wildcard address through loopback.
case "$listen_host" in
  0.0.0.0) check_host=127.0.0.1 ;;
  "[::]") check_host="[::1]" ;;
  *) check_host=$listen_host ;;
esac
base_url="http://$check_host:$listen_port"

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
[ -f "$repo/package.json" ] && [ -f "$repo/src-tauri/Cargo.toml" ] \
  || die "run this script from a Toodo checkout ($repo is not one)"

[ "$(uname -s)" = Linux ] || die "toodo-server is installed on Linux only"

log "Checking prerequisites"
missing=()
for tool in node npm cargo git gh herdr ssh curl; do
  command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
if [ "${#missing[@]}" -gt 0 ]; then
  die "missing on PATH: ${missing[*]}
  node/npm: https://nodejs.org   cargo: https://rustup.rs   gh: https://cli.github.com
  herdr must be installed and running; Toodo never installs it"
fi
if [ "$service" = 1 ]; then
  command -v systemctl >/dev/null 2>&1 || die "systemctl not found; use --no-service to install without a service"
  systemctl --user show-environment >/dev/null 2>&1 \
    || die "no systemd user manager for $USER (systemctl --user failed); log in through a session or use --no-service"
fi

bin_dir=$HOME/.local/bin
share_dir=${XDG_DATA_HOME:-$HOME/.local/share}
web_dir=$share_dir/toodo-server-web
data_dir=$share_dir/toodo-server
unit_dir=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
unit=$unit_dir/toodo-server.service

log "Installing npm dependencies"
(cd "$repo" && npm ci)

log "Building the web app"
(cd "$repo" && npm run build:web)

log "Building toodo-server"
(cd "$repo/src-tauri" && cargo build --release --no-default-features --features server --bin toodo-server)

log "Installing $bin_dir/toodo-server"
mkdir -p "$bin_dir"
tmp_bin=$(mktemp "$bin_dir/.toodo-server.XXXXXX")
cp "$repo/src-tauri/target/release/toodo-server" "$tmp_bin"
chmod 755 "$tmp_bin"
mv -f "$tmp_bin" "$bin_dir/toodo-server"

log "Installing $web_dir"
mkdir -p "$share_dir"
tmp_web=$(mktemp -d "$share_dir/.toodo-server-web.XXXXXX")
cp -R "$repo/dist-web/." "$tmp_web/"
chmod 755 "$tmp_web"
if [ -e "$web_dir" ]; then
  old_web=$(mktemp -d "$share_dir/.toodo-server-web.old.XXXXXX")
  mv "$web_dir" "$old_web/web"
  mv "$tmp_web" "$web_dir"
  rm -rf "$old_web"
else
  mv "$tmp_web" "$web_dir"
fi

if [ "$service" = 0 ]; then
  log "Installed without a service (--no-service). Run it with:"
  echo "    TOODO_LISTEN=$listen TOODO_WEB_DIR=$web_dir $bin_dir/toodo-server"
  exit 0
fi

# The service needs git, ssh, gh, and herdr; take the directories they are found in now.
path_dirs=()
add_path_dir() {
  local dir=$1 seen
  for seen in "${path_dirs[@]+"${path_dirs[@]}"}"; do
    [ "$seen" = "$dir" ] && return 0
  done
  path_dirs+=("$dir")
}
add_path_dir "$bin_dir"
for tool in herdr gh git ssh; do
  add_path_dir "$(dirname "$(command -v "$tool")")"
done
for dir in /usr/local/bin /usr/bin /bin; do
  add_path_dir "$dir"
done
service_path=$(IFS=:; echo "${path_dirs[*]}")

log "Writing $unit"
mkdir -p "$unit_dir"
tmp_unit=$(mktemp "$unit_dir/.toodo-server.service.XXXXXX")
cat >"$tmp_unit" <<EOF
# Written by scripts/install-server.sh; rerun it instead of editing this file.
[Unit]
Description=Toodo server
After=network.target herdr-server.service
Wants=herdr-server.service

[Service]
ExecStart=$bin_dir/toodo-server
Environment=TOODO_LISTEN=$listen
Environment=TOODO_WEB_DIR=$web_dir
Environment=TOODO_DATA_DIR=$data_dir
Environment=PATH=$service_path
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF
chmod 644 "$tmp_unit"
mv -f "$tmp_unit" "$unit"

log "Restarting toodo-server.service"
systemctl --user daemon-reload
systemctl --user enable toodo-server.service >/dev/null
systemctl --user restart toodo-server.service

log "Waiting for $base_url"
up=0
for _ in $(seq 1 60); do
  if curl -s -o /dev/null --max-time 2 "$base_url/"; then
    up=1
    break
  fi
  systemctl --user is-active --quiet toodo-server.service || break
  sleep 0.5
done
if [ "$up" = 0 ]; then
  systemctl --user status --no-pager toodo-server.service >&2 || true
  die "toodo-server did not answer on $base_url; see journalctl --user -u toodo-server"
fi

status_of() { curl -s -o /dev/null --max-time 5 -w '%{http_code}' "$@"; }
web_status=$(status_of "$base_url/")
[ "$web_status" = 200 ] || die "GET $base_url/ returned $web_status, expected 200"
api_status=$(status_of -X POST -H 'Content-Type: application/json' -d '{}' "$base_url/api/rpc/list_tasks")
[ "$api_status" = 401 ] || die "POST $base_url/api/rpc/list_tasks without a token returned $api_status, expected 401"
log "Health check passed: GET / -> 200, /api without a token -> 401"

if [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null || true)" != yes ]; then
  echo "note: lingering is off, so the service stops when you log out; run: loginctl enable-linger $USER"
fi

cat <<EOF

toodo-server is running on $base_url
  Web token (the browser asks for it once): $data_dir/web-token
  MCP token (for agents, /mcp):             $data_dir/mcp-token
  Logs: journalctl --user -u toodo-server
Keep it on loopback; reach it with e.g. ssh -L $listen_port:127.0.0.1:$listen_port <this host>.
EOF
