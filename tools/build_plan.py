# /// script
# requires-python = ">=3.10"
# ///
"""Print the cargo target directory and cdylib name for a package."""

import json
import subprocess
import sys

manifest, wanted = sys.argv[1:]
metadata = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--manifest-path", manifest],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
)
package = next((item for item in metadata["packages"] if item["name"] == wanted), None)
if package is None:
    sys.exit(f"package not in metadata: {wanted}")
library = next(
    (target["name"] for target in package["targets"] if "cdylib" in target.get("crate_types", [])),
    None,
)
if library is None:
    sys.exit(f"package has no cdylib target: {wanted}")
print(metadata["target_directory"])
print(library)
