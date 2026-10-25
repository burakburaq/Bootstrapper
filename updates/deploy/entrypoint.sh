#!/bin/sh
set -eu
if [ -r /run/secrets/database_url ]; then export DATABASE_URL="$(cat /run/secrets/database_url)"; fi
if [ -r /run/secrets/r2_access ]; then export R2_ACCESS_KEY_ID="$(cat /run/secrets/r2_access)"; fi
if [ -r /run/secrets/r2_secret ]; then export R2_SECRET_ACCESS_KEY="$(cat /run/secrets/r2_secret)"; fi
if [ -r /run/secrets/smoke_tokens ]; then export UPDATE_SMOKE_TOKENS="$(cat /run/secrets/smoke_tokens)"; fi
if [ -r /run/secrets/install_authorization_key ]; then export UPDATE_AUTHORIZATION_KEY_FILE=/run/secrets/install_authorization_key; fi
exec /usr/local/bin/update-server "$@"
