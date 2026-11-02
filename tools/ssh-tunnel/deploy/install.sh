#!/usr/bin/env bash
# Author: Ray Zhang
# Run as root on the explicitly selected relay host. No LabSlot processes restart.
set -Eeuo pipefail
if [[ $# != 2 || ! $1 =~ ^[0-9a-f]{64}$ || ! $2 =~ ^[a-zA-Z0-9._-]+$ ]]; then
    echo 'Usage: install.sh BINARY_SHA256 RELEASE_ID' >&2
    exit 2
fi
[[ $(id -u) == 0 ]] || { echo 'root required' >&2; exit 2; }
base=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
expected=$1
release=$2
printf '%s  %s\n' "$expected" "$base/ry-ssh-tunnel" | sha256sum --check --status
nginx -t
config=/etc/nginx/conf.d/labslot.conf
[[ -f $config ]] || { echo 'Expected TLS virtual-host config missing' >&2; exit 2; }
[[ ! -e /opt/ry-ssh-tunnel/releases/$release ]] || { echo 'Release already exists; inspect before retrying' >&2; exit 2; }
backup=/var/backups/ry-ssh-tunnel/$(date -u +%Y%m%dT%H%M%SZ)-$$
install -d -m 0700 "$backup"
cp -a "$config" "$backup/labslot.conf"
files=(/etc/systemd/system/ry-ssh-tunnel.service /etc/nginx/snippets/ry-ssh-tunnel.inc /etc/nginx/conf.d/ry-ssh-tunnel-rate.conf)
for i in "${!files[@]}"; do
    [[ ! -e ${files[$i]} ]] || cp -a "${files[$i]}" "$backup/file-$i"
done
old_current=$(readlink /opt/ry-ssh-tunnel/current || true)
was_active=$(systemctl is-active ry-ssh-tunnel.service || true)
was_enabled=$(systemctl is-enabled ry-ssh-tunnel.service 2>/dev/null || true)
changed=0
rollback() {
    rc=$1
    trap - ERR HUP INT TERM
    set +e
    if [[ $changed == 1 ]]; then
        systemctl stop ry-ssh-tunnel.service || true
        if [[ $was_enabled != enabled ]]; then systemctl disable ry-ssh-tunnel.service >/dev/null 2>&1 || true; fi
        cp -a "$backup/labslot.conf" "$config"
        for i in "${!files[@]}"; do
            if [[ -e $backup/file-$i ]]; then cp -a "$backup/file-$i" "${files[$i]}";
            else rm -f -- "${files[$i]}"; fi
        done
        if [[ -n $old_current ]]; then ln -sfn -- "$old_current" /opt/ry-ssh-tunnel/current;
        else rm -f -- /opt/ry-ssh-tunnel/current; fi
        systemctl daemon-reload
        if [[ $was_active == active ]]; then systemctl start ry-ssh-tunnel.service || true; fi
        nginx -t && systemctl reload nginx || true
    fi
    echo "Deployment failed; rollback attempted. Inspect $backup and retained release." >&2
    exit "$rc"
}
trap 'rollback $?' ERR
trap 'rollback 129' HUP
trap 'rollback 130' INT
trap 'rollback 143' TERM
if ! id ry-ssh-tunnel >/dev/null 2>&1; then useradd --system --no-create-home --shell /usr/sbin/nologin ry-ssh-tunnel; fi
install -d -m 0755 /opt/ry-ssh-tunnel/releases /etc/nginx/snippets
install -d -m 0755 "/opt/ry-ssh-tunnel/releases/$release"
install -m 0755 "$base/ry-ssh-tunnel" "/opt/ry-ssh-tunnel/releases/$release/ry-ssh-tunnel"
changed=1
ln -sfn "/opt/ry-ssh-tunnel/releases/$release" /opt/ry-ssh-tunnel/current
install -m 0644 "$base/ry-ssh-tunnel.service" /etc/systemd/system/ry-ssh-tunnel.service
install -m 0644 "$base/ry-ssh-tunnel.nginx.inc" /etc/nginx/snippets/ry-ssh-tunnel.inc
printf '%s\n' 'limit_req_zone $binary_remote_addr zone=ry_ssh_tunnel:1m rate=5r/s;' > /etc/nginx/conf.d/ry-ssh-tunnel-rate.conf
python3 - "$config" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
text = p.read_text()
include = '    include /etc/nginx/snippets/ry-ssh-tunnel.inc;'
anchor = '    include /etc/nginx/snippets/labslot-security-probes.inc;'
if include not in text:
    if text.count(anchor) != 1:
        raise SystemExit('TLS vhost anchor changed; refusing ambiguous modification')
    p.write_text(text.replace(anchor, anchor + '\n' + include, 1))
PY
nginx -t
systemctl daemon-reload
systemctl enable ry-ssh-tunnel.service
systemctl restart ry-ssh-tunnel.service
for attempt in {1..15}; do
    if curl --fail --silent --max-time 2 http://127.0.0.1:50122/healthz > "$backup/health.json"; then break; fi
    sleep 1
done
curl --fail --silent --max-time 3 http://127.0.0.1:50122/healthz
systemctl reload nginx
public_ready=0
for attempt in {1..5}; do
    # Existing keep-alive workers may briefly serve the old route during reload.
    if public_health=$(curl --fail --silent --max-time 10 https://nmrkit.ciqtek.com/ssh-tunnel/healthz) && [[ $public_health == 'ry-ssh-tunnel ok' ]]; then
        public_ready=1
        break
    fi
    sleep 1
done
[[ $public_ready == 1 ]]
printf '%s\n' "$public_health"
trap - ERR HUP INT TERM
printf '\nrelease=%s\nsha256=%s\nbackup=%s\n' "$release" "$expected" "$backup"
systemctl show ry-ssh-tunnel.service --property=MainPID --property=ActiveState --property=SubState
