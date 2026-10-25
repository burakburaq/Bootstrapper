"""Fetch the already-built image from a successful manual full CI run."""

import hashlib
import io
import json
import os
from pathlib import Path
import re
from urllib.request import HTTPRedirectHandler, Request, build_opener
import zipfile


class ArtifactRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        # Artifact storage redirects use their own signed URL, not the API token.
        if redirected is not None:
            redirected.remove_header("Authorization")
        return redirected


def main():
    repo = os.environ["GITHUB_REPOSITORY"]
    run_id, sha = os.environ["BUILD_RUN_ID"], os.environ["SOURCE_SHA"]
    if not run_id.isdecimal() or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise SystemExit("Use a numeric successful CI run ID and exact source SHA.")
    api = f"https://api.github.com/repos/{repo}"

    def get(path):
        request = Request(api + path, headers={
            "Authorization": "Bearer " + os.environ["GH_TOKEN"],
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        })
        with build_opener(ArtifactRedirect()).open(request, timeout=120) as response:
            return response.read()

    run = json.loads(get(f"/actions/runs/{run_id}"))
    if run["conclusion"] != "success" or run["event"] != "workflow_dispatch" or run["path"] != ".github/workflows/ci.yml" or run["head_branch"] != "main":
        raise SystemExit("Only a successful manually dispatched full CI run can be published.")
    artifacts = json.loads(get(f"/actions/runs/{run_id}/artifacts?per_page=100"))["artifacts"]
    matches = [item for item in artifacts if item["name"] == f"update-server-build-{sha}" and not item["expired"]]
    if len(matches) != 1:
        raise SystemExit("Exactly one unexpired image artifact for this SHA is required.")
    archive = get(f"/actions/artifacts/{matches[0]['id']}/zip")
    expected = {"source-sha.txt", "update-server-image.sha256", "update-server-image.tar"}
    with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
        if set(zipped.namelist()) != expected:
            raise SystemExit("Unexpected artifact file layout.")
        for name in expected:
            Path(name).write_bytes(zipped.read(name))
    if Path("source-sha.txt").read_text().strip() != sha:
        raise SystemExit("Artifact source SHA differs from the selected SHA.")
    expected_hash, filename = Path("update-server-image.sha256").read_text().split()
    if filename != "update-server-image.tar":
        raise SystemExit("Unexpected checksum filename.")
    with open(filename, "rb") as image:
        digest = hashlib.file_digest(image, "sha256").hexdigest()
    if digest != expected_hash:
        raise SystemExit("Image archive hash mismatch.")
    print(f"Verified existing cloud image from run {run_id}, SHA {sha}.")


if __name__ == "__main__":
    main()
