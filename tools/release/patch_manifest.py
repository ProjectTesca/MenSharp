"""Stamps the release version and zip URL into the staged package.json.

Environment: VERSION, ZIP_URL. Run from the repository root by release.yml.
"""

import json
import os

with open("staging/package.json") as f:
    manifest = json.load(f)
manifest["version"] = os.environ["VERSION"]
manifest["url"] = os.environ["ZIP_URL"]
with open("staging/package.json", "w") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
