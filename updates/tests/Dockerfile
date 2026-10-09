FROM python:3.13-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends jq util-linux && rm -rf /var/lib/apt/lists/*
WORKDIR /work
COPY deploy/deploy.sh deploy/deploy.sh
COPY tests/deploy_test.py tests/deploy_test.py
RUN python3 tests/deploy_test.py
