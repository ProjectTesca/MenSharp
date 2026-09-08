"""Builds the VPM repository listing (pages/index.json) for GitHub Pages.

Environment: VERSION, SHA256, REPOSITORY (owner/repo). Run from the
repository root by release.yml, after patch_manifest.py.

Currently lists only the release being published; per-release history can be
added later by merging the previously deployed index.json.
"""

import json
import os

version = os.environ["VERSION"]
owner, repo = os.environ["REPOSITORY"].split("/")
pages_url = f"https://{owner.lower()}.github.io/{repo}"

with open("staging/package.json") as f:
    manifest = json.load(f)
manifest["zipSHA256"] = os.environ["SHA256"]

listing = {
    "name": "MenSharp",
    "id": "io.tesca.mensharp.repo",
    "url": f"{pages_url}/index.json",
    "author": "ProjectTesca",
    "packages": {
        "io.tesca.mensharp": {
            "versions": {version: manifest},
        },
    },
}

os.makedirs("pages", exist_ok=True)
with open("pages/index.json", "w") as f:
    json.dump(listing, f, indent=2)
    f.write("\n")
with open("pages/index.html", "w") as f:
    f.write(
        "<h1>MenSharp VPM repository</h1>"
        f"<p>Add <code>{pages_url}/index.json</code> to ALCOM or the "
        "VRChat Creator Companion.</p>\n"
    )
