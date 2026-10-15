"""Select checks for this independent installer repository."""
import json
import os
import subprocess

def select(paths):
    ui = native = updates = False
    for path in paths:
        if path.endswith(".md") or path.startswith(("docs/", "preview")):
            continue
        if path.startswith("updates/"):
            updates = True
            native |= path.startswith(("updates/crates/", "updates/Cargo"))
        elif path.startswith(("ui/", "scripts/", "examples/")) or path.startswith(("package", "tsconfig")):
            ui = native = True
        elif path.startswith("src-tauri/"):
            ui = native = True
        else:
            ui = native = updates = True
    return dict(ui=ui, native=native, updates=updates)

if __name__ == "__main__":
    event = json.load(open(os.environ["GITHUB_EVENT_PATH"], encoding="utf-8"))
    if "pull_request" in event:
        base = event["pull_request"]["base"]["sha"]
        head = event["pull_request"]["head"]["sha"]
        paths = subprocess.check_output(["git", "diff", "--name-only", "--no-renames", f"{base}...{head}"]).decode().splitlines()
        result = select(paths)
    else:
        result = dict(ui=True, native=True, updates=True)
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for name, enabled in result.items():
            output.write(f"{name}={str(enabled).lower()}\n")
