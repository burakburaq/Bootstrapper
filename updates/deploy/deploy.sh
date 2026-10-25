#!/usr/bin/env bash
set -Eeuo pipefail
# Linux VPS. This directory is a standalone update deployment; no application Compose file is touched.
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
for command in docker curl jq flock sha256sum; do command -v "$command" >/dev/null || { echo "Required: $command" >&2; exit 1; }; done
mkdir -p state/routing
exec 9>state/deploy.lock
flock -n 9 || { echo 'Another update deployment is running.' >&2; exit 1; }
touch state/images.env
compose=(docker compose --env-file deploy.env --env-file state/images.env -f compose.yml)
authorization=${UPDATE_INSTALL_AUTHORIZATION:-$(awk -F= '$1 == "UPDATE_INSTALL_AUTHORIZATION" { value=$2 } END { gsub(/\r/, "", value); print value }' deploy.env)}
[[ ${authorization:-0} == 0 || $authorization == 1 ]] || { echo 'UPDATE_INSTALL_AUTHORIZATION must be 0 or 1.' >&2; exit 1; }
if [[ ${authorization:-0} == 1 ]]; then
    [[ -s secrets/install-authorization.key ]] || { echo 'Missing install authorization key.' >&2; exit 1; }
    compose+=(-f compose.authorization.yml)
fi
mode=${1:-}
image=${2:-}
[[ $mode == deploy || $mode == rollback || $mode == bootstrap ]] || { echo 'Usage: deploy.sh bootstrap|deploy|rollback registry/image@sha256:DIGEST' >&2; exit 1; }
[[ $image =~ ^[a-z0-9][a-z0-9./:_-]*@sha256:[a-f0-9]{64}$ ]] || { echo 'An immutable image digest is required.' >&2; exit 1; }
observe=${UPDATE_OBSERVATION_SECONDS:-30}
drain=${UPDATE_DRAIN_SECONDS:-120}
[[ $observe =~ ^[0-9]+$ && $drain =~ ^[0-9]+$ ]] || exit 1
public_url=$(jq -er '.public_url' config.json)
[[ $public_url == https://* ]] || { echo 'HTTPS public_url required.' >&2; exit 1; }
deployment_id=$(date -u +%Y%m%dT%H%M%SZ)-$$
config_revision=$(sha256sum config.json | cut -d' ' -f1)
old=none
candidate=blue
schema=unknown
record() {
    local temporary
    temporary=$(mktemp state/deployment.XXXXXX)
    jq -n --arg id "$deployment_id" --arg phase "$1" --arg old "$old" --arg candidate "$candidate" \
        --arg image "$image" --arg schema "$schema" --arg config "$config_revision" --arg at "$(date -u +%FT%TZ)" \
        '{deployment_id:$id,phase:$phase,previous_slot:$old,candidate_slot:$candidate,image:$image,schema:$schema,config_revision:$config,updated_at:$at}' >"$temporary"
    mv -- "$temporary" state/deployment.json
    jq -c . state/deployment.json >>state/history.jsonl
}
active() {
    runtime 'show map /etc/haproxy/maps/active.map' | awk '
        $2 == "active" && ($3 == "blue" || $3 == "green") { value=$3; count++ }
        END { if (count == 1) print value; else exit 1 }'
}
runtime() {
    # The admin socket is loopback-only inside the router; commands contain validated colors/IPs.
    "${compose[@]}" exec -T router sh -c 'printf "%s\n" "$1" | nc -w 2 127.0.0.1 9999' sh "$1"
}
route_file() {
    local temporary
    temporary=$(mktemp state/routing/route.XXXXXX)
    printf 'active %s\n' "$1" >"$temporary"
    mv -- "$temporary" state/routing/active.map
}
switch() {
    local destination=$1 previous=$2
    route_file "$destination"
    local response
    if ! response=$(runtime "set map /etc/haproxy/maps/active.map active $destination") || [[ -n ${response//[[:space:]]/} ]]; then
        route_file "$previous"
        echo 'Router rejected the candidate; the previous map is retained.' >&2
        return 1
    fi
    # Confirm actual live configuration, not merely the on-disk state.
    [[ $(active) == "$destination" ]]
}
port() { if [[ $1 == blue ]]; then printf 18081; else printf 18082; fi; }
healthy() { curl -fsS --max-time 15 "http://127.0.0.1:$(port "$1")/health/ready" | jq -e --arg slot "$1" '.ready == true and .instance == $slot' >/dev/null; }
external() { curl -fsS --max-time 15 "${public_url%/}/ops/live" | jq -e --arg slot "$1" '.instance == $slot' >/dev/null; }
smoke() {
    local flags=()
    [[ $mode == bootstrap ]] || flags+=(--require-artifacts)
    "${compose[@]}" exec -T "update-server-$1" /usr/local/bin/update-entrypoint --config /etc/update/config.json \
        smoke --origin "$2" "${flags[@]}"
}
recover_old() {
    if [[ $old != none ]] && healthy "$old" && switch "$old" "$candidate"; then
        record rolled_back
        echo "Traffic returned to $old; candidate remains available for inspection." >&2
    else
        record recovery_required
        echo 'Automatic rollback unavailable. Both slots are retained; inspect actual proxy configuration.' >&2
    fi
}
interrupted() { record interrupted; echo 'Deployment interrupted. Next run reconciles the live proxy route; neither slot is stopped.' >&2; }
trap interrupted INT TERM

if [[ $mode == bootstrap ]]; then
    # Initial provisioning only: refuse to bootstrap over any already-running proxy.
    [[ -z $("${compose[@]}" ps --status running -q proxy) ]] || { echo 'Proxy already exists; use deploy or rollback.' >&2; exit 1; }
    "${compose[@]}" up -d --wait postgres
else
    old=$(active) || { echo 'Cannot establish actual proxy route; refusing to stop or replace any slot.' >&2; exit 1; }
    healthy "$old" || { echo 'Active slot is unhealthy; recovery must be inspected before deployment.' >&2; exit 1; }
    route_file "$old" # Restore disk state after an interrupted or rejected map change.
    if [[ $old == blue ]]; then candidate=green; else candidate=blue; fi
fi
record preparing
# Refuse to replace an inactive slot that still has outstanding requests.
if [[ -n $("${compose[@]}" ps --status running -q "update-server-$candidate") ]]; then
    requests=$(curl -fsS --max-time 15 "http://127.0.0.1:$(port "$candidate")/health/inflight" | jq -er '.inflight')
    [[ $requests == 0 ]] || { record cleanup_deferred; echo 'Candidate slot still has requests. No slot was replaced.' >&2; exit 1; }
fi
temporary=$(mktemp state/images.XXXXXX)
grep -v "^UPDATE_${candidate^^}_IMAGE=" state/images.env >"$temporary" || true
printf 'UPDATE_%s_IMAGE=%s\n' "${candidate^^}" "$image" >>"$temporary"
mv -- "$temporary" state/images.env
export UPDATE_MIGRATION_IMAGE=$image
"${compose[@]}" pull "update-server-$candidate" migrate
if [[ $mode != rollback ]]; then
    # SQLx advisory migration lock and the deployment lock serialize migration execution.
    "${compose[@]}" run --rm --no-deps migrate
    "${compose[@]}" exec -T postgres psql -U update_migrator -d updates -v ON_ERROR_STOP=1 -f /ops/grants.sql
fi
schema=$("${compose[@]}" exec -T postgres psql -U update_migrator -d updates -Atc 'SELECT max(version) FROM _sqlx_migrations WHERE success')
record migrated
"${compose[@]}" up -d --no-deps --wait --wait-timeout 180 "update-server-$candidate"
healthy "$candidate"
smoke "$candidate" http://127.0.0.1:8080
record candidate_ready
if [[ $old == none ]]; then
    route_file "$candidate"
    "${compose[@]}" up -d --no-deps router proxy
    deadline=$((SECONDS + 30))
    until [[ $(active 2>/dev/null) == "$candidate" ]]; do
        (( SECONDS < deadline )) || { record proxy_not_ready; exit 1; }
        sleep 1
    done
    [[ $(active) == "$candidate" ]]
else
    # Pin the ready candidate's actual IP before switching, independent of DNS refresh timing.
    candidate_id=$("${compose[@]}" ps -q "update-server-$candidate")
    [[ -n $candidate_id ]] || { record candidate_missing; exit 1; }
    candidate_ip=$(docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$candidate_id")
    [[ $candidate_ip =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || { record invalid_candidate_address; exit 1; }
    response=$(runtime "set server $candidate/slot addr $candidate_ip port 8080")
    [[ -z ${response//[[:space:]]/} || $response == *"IP changed"* || $response == *"no need to change"* ]] || { record candidate_address_rejected; exit 1; }
    switch "$candidate" "$old" || { record proxy_rejected; exit 1; }
fi
record switched
if [[ $mode == bootstrap ]]; then
    deadline=$((SECONDS + 120))
    until external "$candidate" 2>/dev/null; do
        (( SECONDS < deadline )) || { record first_tls_not_ready; exit 1; }
        sleep 2
    done
fi
if ! external "$candidate" || ! smoke "$candidate" "${public_url%/}"; then recover_old; exit 1; fi
deadline=$((SECONDS + observe))
while (( SECONDS < deadline )); do
    if ! healthy "$candidate" || ! external "$candidate"; then recover_old; exit 1; fi
    sleep 2
done
record observed
if [[ $old != none ]]; then
    deadline=$((SECONDS + drain))
    idle=0
    while (( SECONDS < deadline )); do
        count=$(curl -fsS --max-time 15 "http://127.0.0.1:$(port "$old")/health/inflight" | jq -er '.inflight') || break
        if [[ $count == 0 ]]; then idle=$((idle+1)); else idle=0; fi
        # Consecutive idle probes cover a request racing with proxy configuration replacement.
        if (( idle >= 3 )); then
            "${compose[@]}" stop --timeout -1 "update-server-$old"
            record completed
            echo "Update server active: $candidate ($image)"
            exit 0
        fi
        sleep 2
    done
    record cleanup_deferred
    echo "Traffic uses $candidate. Old slot retained because draining could not be confirmed." >&2
else
    record completed
    echo "Update server provisioned: $candidate ($image)"
fi
