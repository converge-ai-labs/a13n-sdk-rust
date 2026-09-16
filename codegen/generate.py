"""Generate the Rust SDK from its pinned local Service contract.

Generation does not import, check out, or execute the Service. Check mode compares
both bytes and file names without replacing the committed generated directory.
"""

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "codegen"
TARGET = ROOT / "src/generated"


def run(*args: str, cwd: Path = ROOT) -> None:
    subprocess.run(args, cwd=cwd, check=True)


def prepare(document: dict) -> dict:
    """Generator-only annotations; the published 3.1 contract stays unchanged."""
    document = json.loads(json.dumps(document))
    schemas = document["components"]["schemas"]

    def unrestricted(schema: dict) -> bool:
        if "$ref" in schema:
            schema = schemas[schema["$ref"].split("/")[-1]]
        return not schema.keys() - {"title", "description", "default", "x-rust-type"}

    def visit(value: object) -> None:
        if isinstance(value, dict):
            content = value.get("requestBody", {}).get("content", {})
            if len(content) == 1:
                media_type, media = next(iter(content.items()))
                if media.get("schema", {}).get("format") == "binary":
                    value["x-rust-binary-content-type"] = media_type
            branches = value.get("anyOf", [])
            if any(unrestricted(branch) for branch in branches):
                value["x-rust-type"] = "serde_json::Value"
            if value.get("type") == "boolean" and isinstance(value.get("const"), bool):
                value["x-rust-boolean-const"] = True
                value["x-rust-boolean-true"] = value["const"]
                value["x-rust-boolean-value"] = str(value["const"]).lower()
            if value.get("type") == "null":
                value["x-rust-type"] = "()"
            if any(prop.get("writeOnly") for prop in value.get("properties", {}).values()):
                value["x-rust-sensitive"] = True
            for name, prop in value.get("properties", {}).items():
                if prop.get("type") == "null" and name not in value.get("required", []):
                    prop["x-rust-null-only"] = True
            for child in list(value.values()):
                visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)

    visit(document)
    schemas["RunStatus"]["x-rust-unknown-enum"] = True
    return document


def generate(document: dict, work: Path) -> Path:
    source = work / "openapi.json"
    source.write_text(json.dumps(prepare(document)))
    output = work / "output"
    raw = work / "rust"
    run(
        "uv",
        "tool",
        "run",
        "--from",
        "openapi-generator-cli[jdk4py]==7.25.0",
        "openapi-generator-cli",
        "generate",
        "-i",
        str(source),
        "-g",
        "rust",
        "-t",
        str(CONFIG / "templates"),
        "-o",
        str(raw),
        "--additional-properties=hideGenerationTimestamp=true",
        "--global-property=apiDocs=false,modelDocs=false,apiTests=false,modelTests=false",
    )
    shutil.copytree(raw / "src", output)
    (output / "lib.rs").rename(output / "mod.rs")
    for path in output.rglob("*.rs"):
        path.write_text(
            path.read_text()
            .replace("crate::models", "crate::generated::models")
            .replace("crate::apis", "crate::generated::apis")
            .replace("use crate::{", "use crate::generated::{")
        )
    run("rustfmt", "--edition", "2024", str(output / "mod.rs"))
    return output


def files(path: Path) -> dict[str, bytes]:
    return {
        str(file.relative_to(path)): file.read_bytes()
        for file in path.rglob("*")
        if file.is_file() and "__pycache__" not in file.parts
    }


def install(output: Path, target: Path, *, check: bool) -> bool:
    actual, expected = files(target), files(output)
    changed = sorted(name for name in actual.keys() | expected.keys() if actual.get(name) != expected.get(name))
    if not changed:
        return True
    if check:
        print(f"Stale generated files in {target.relative_to(ROOT)}: " + ", ".join(changed[:20]))
        return False
    # Only generator-owned directories are replaced, including removed schemas.
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(output, target)
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    document = json.loads((ROOT / "contract/openapi.json").read_text())
    with tempfile.TemporaryDirectory(prefix="a13n-codegen-") as temp:
        output = generate(document, Path(temp))
        if not install(output, TARGET, check=args.check):
            parser.exit(1, "SDK bindings changed. Run make generate and commit the result.\n")


if __name__ == "__main__":
    main()
