"""Local contract provenance, generation adapters, and output ownership."""

import hashlib
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("sdk_codegen", ROOT / "codegen/generate.py")
assert SPEC and SPEC.loader
codegen = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(codegen)


def test_contract_source_matches_pinned_bytes() -> None:
    source = json.loads((ROOT / "contract/source.json").read_text())
    assert source["repository"] == "converge-ai-labs/agent-foundation"
    assert len(source["commit"]) == 40
    assert all(char in "0123456789abcdef" for char in source["commit"])
    vendored = {
        str(path.relative_to(ROOT / "contract"))
        for path in (ROOT / "contract").rglob("*")
        if path.is_file() and str(path.relative_to(ROOT / "contract")) not in {"source.json", "README.md"}
    }
    assert set(source["files"]) == vendored
    for name, entry in source["files"].items():
        assert hashlib.sha256((ROOT / "contract" / name).read_bytes()).hexdigest() == entry["sha256"]


def test_drift_checks_content_and_stale_files_without_mutation(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(codegen, "ROOT", tmp_path)
    target, output = tmp_path / "committed", tmp_path / "regenerated"
    target.mkdir()
    output.mkdir()
    (target / "old.py").write_text("old")
    (target / "current.py").write_text("out of date")
    (output / "current.py").write_text("current")
    before = codegen.files(target)
    assert not codegen.install(output, target, check=True)
    assert codegen.files(target) == before
    assert codegen.install(output, target, check=False)
    assert codegen.files(target) == {"current.py": b"current"}
    assert codegen.install(output, target, check=True)


def test_missing_output_is_drift_without_creating_it(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(codegen, "ROOT", tmp_path)
    output = tmp_path / "regenerated"
    output.mkdir()
    (output / "new.py").write_text("new")
    target = tmp_path / "missing"
    assert not codegen.install(output, target, check=True)
    assert not target.exists()


def test_all_native_operations_have_bindings() -> None:
    document = json.loads((ROOT / "contract/openapi.json").read_text())
    generated = "\n".join(path.read_text() for path in (ROOT / "src/generated/apis").glob("*.rs"))
    for path in document["paths"].values():
        for method, operation in path.items():
            if method in {"get", "post", "patch", "put", "delete", "head", "options"}:
                assert f"pub async fn {operation['operationId']}(" in generated


def test_adapter_keeps_typed_unions_and_contract_unchanged() -> None:
    document = json.loads((ROOT / "contract/openapi.json").read_text())
    before = json.dumps(document)
    adapted = codegen.prepare(document)
    assert json.dumps(document) == before
    schemas = adapted["components"]["schemas"]
    for name in ["ActorRef", "EnvironmentSelection"]:
        assert schemas[name] == document["components"]["schemas"][name]
    assert schemas["RunStatus"]["x-rust-unknown-enum"]
    assert "x-rust-unknown-enum" not in schemas["PrincipalType"]


def test_schema_adapters_preserve_presence_constants_binary_and_sensitive_data() -> None:
    document = {
        "components": {
            "schemas": {
                "RunStatus": {"type": "string"},
                "Arbitrary": {},
                "OpenUnion": {"anyOf": [{"$ref": "#/components/schemas/Arbitrary"}, {"type": "string"}]},
                "Constant": {"type": "boolean", "const": False},
                "Object": {
                    "type": "object",
                    "properties": {
                        "secret": {"type": "string", "writeOnly": True},
                        "empty": {"type": "null"},
                    },
                },
            }
        },
        "paths": {
            "/binary": {
                "put": {
                    "requestBody": {
                        "content": {
                            "image/webp": {"schema": {"type": "string", "format": "binary"}},
                        }
                    }
                }
            }
        },
    }
    adapted = codegen.prepare(document)
    schemas = adapted["components"]["schemas"]
    assert schemas["OpenUnion"]["x-rust-type"] == "serde_json::Value"
    assert schemas["Constant"]["x-rust-boolean-value"] == "false"
    assert schemas["Object"]["x-rust-sensitive"]
    assert schemas["Object"]["properties"]["empty"]["x-rust-null-only"]
    assert schemas["Object"]["properties"]["empty"]["x-rust-type"] == "()"
    assert adapted["paths"]["/binary"]["put"]["x-rust-binary-content-type"] == "image/webp"


def test_cli_has_independent_workspace_and_is_excluded_from_sdk_package() -> None:
    import subprocess
    import tomllib

    sdk = tomllib.loads((ROOT / "Cargo.toml").read_text())
    cli = tomllib.loads((ROOT / "a13n-service-cli/Cargo.toml").read_text())
    assert cli["dependencies"]["a13n"] == {"path": ".."}
    assert cli["package"]["publish"] is False
    assert "a13n-service-cli/**" in sdk["package"]["exclude"]
    for manifest in [ROOT / "Cargo.toml", ROOT / "a13n-service-cli/Cargo.toml"]:
        output = subprocess.check_output(
            [
                "cargo",
                "metadata",
                "--locked",
                "--no-deps",
                "--format-version",
                "1",
                "--manifest-path",
                str(manifest),
            ],
            cwd=ROOT,
            text=True,
        )
        metadata = json.loads(output)
        assert Path(metadata["workspace_root"]).resolve() == manifest.parent.resolve()
        assert len(metadata["workspace_members"]) == 1
    package = subprocess.check_output(["cargo", "package", "--locked", "--allow-dirty", "--list"], cwd=ROOT, text=True)
    assert not any(name.startswith("a13n-service-cli/") for name in package.splitlines())


def test_changed_http_contract_regenerates_bindings(tmp_path: Path) -> None:
    """Exercise the real pinned generator, not the sync test's fake make."""
    document = {
        "openapi": "3.1.0",
        "info": {"title": "Autogen fixture", "version": "1"},
        "components": {"schemas": {"RunStatus": {"type": "string", "enum": ["queued", "running"]}}},
        "paths": {
            "/api/v1/autogen-probe": {
                "get": {"operationId": "autogen_probe", "responses": {"204": {"description": "No content"}}}
            }
        },
    }
    initial, updated = tmp_path / "initial", tmp_path / "updated"
    initial.mkdir()
    updated.mkdir()
    before = codegen.files(codegen.generate(document, initial))
    document["paths"]["/api/v1/autogen-probe"]["get"]["parameters"] = [
        {"name": "autogen_probe_value", "in": "query", "schema": {"type": "string"}}
    ]
    source = json.dumps(document)
    output = codegen.generate(document, updated)
    after = codegen.files(output)
    assert before != after
    assert not any(b"autogen_probe_value" in value for value in before.values())
    assert any(b"autogen_probe_value" in value for value in after.values())
    assert json.dumps(document) == source
    target = tmp_path / "installed"
    assert codegen.install(output, target, check=False)
    assert codegen.install(output, target, check=True)
