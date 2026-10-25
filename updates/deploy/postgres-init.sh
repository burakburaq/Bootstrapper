#!/bin/sh
set -eu
runtime_password="$(cat /run/secrets/runtime_password)"
psql --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" --set ON_ERROR_STOP=1 --set runtime_password="$runtime_password" <<'SQL'
SELECT format('CREATE ROLE update_runtime LOGIN PASSWORD %L', :'runtime_password') \gexec
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO update_runtime;
SQL
