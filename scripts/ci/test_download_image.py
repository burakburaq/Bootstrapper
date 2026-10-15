import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.request import Request
import zipfile

from download_image import ArtifactRedirect, main


class Response(io.BytesIO):
    pass


class ArtifactTests(unittest.TestCase):
    def run_download(self, conclusion="success", stored_sha=None, corrupt=False, branch="main"):
        sha = "a" * 40
        image = b"already-built-cloud-image"
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as zipped:
            zipped.writestr("source-sha.txt", stored_sha or sha)
            zipped.writestr("update-server-image.sha256", hashlib.sha256(image).hexdigest() + "  update-server-image.tar\n")
            zipped.writestr("update-server-image.tar", b"changed-after-test" if corrupt else image)
        replies = [
            json.dumps(dict(conclusion=conclusion, event="workflow_dispatch", path=".github/workflows/ci.yml", head_branch=branch)).encode(),
            json.dumps(dict(artifacts=[dict(name=f"update-server-build-{sha}", expired=False, id=1)])).encode(),
            archive.getvalue(),
        ]
        with tempfile.TemporaryDirectory() as directory:
            original = os.getcwd()
            os.chdir(directory)
            try:
                with patch.dict(os.environ, GITHUB_REPOSITORY="owner/repo", BUILD_RUN_ID="123", SOURCE_SHA=sha, GH_TOKEN="fixture-token"), \
                        patch("download_image.build_opener") as opener:
                    opener.return_value.open.side_effect = [Response(reply) for reply in replies]
                    main()
                    self.assertEqual(Path("update-server-image.tar").read_bytes(), image)
            finally:
                os.chdir(original)

    def test_publishes_verified_existing_bytes(self):
        self.run_download()

    def test_failed_ci_is_rejected(self):
        with self.assertRaisesRegex(SystemExit, "successful"):
            self.run_download(conclusion="failure")

    def test_unmerged_workflow_cannot_supply_release_artifact(self):
        with self.assertRaisesRegex(SystemExit, "successful"):
            self.run_download(branch="unreviewed-workflow")

    def test_wrong_source_and_changed_package_are_rejected(self):
        with self.assertRaisesRegex(SystemExit, "source SHA"):
            self.run_download(stored_sha="b" * 40)
        with self.assertRaisesRegex(SystemExit, "hash mismatch"):
            self.run_download(corrupt=True)

    def test_storage_redirect_does_not_receive_api_token(self):
        request = Request("https://api.github.com/artifact", headers={"Authorization": "Bearer fixture-token"})
        redirected = ArtifactRedirect().redirect_request(request, None, 302, "Found", {}, "https://storage.example/artifact?signature=fixture")
        self.assertIsNone(redirected.get_header("Authorization"))


if __name__ == "__main__":
    unittest.main()
