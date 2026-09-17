"""Generator adapters and generated binding behavior."""

import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("sdk_codegen", ROOT / "codegen/generate.py")
assert SPEC and SPEC.loader
codegen = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(codegen)


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
    document["paths"]["/api/v1/autogen-probe"]["get"]["parameters"] = [
        {"name": "autogen_probe_value", "in": "query", "schema": {"type": "string"}}
    ]
    output = codegen.generate(document, tmp_path)
    target = tmp_path / "installed"
    target.mkdir()
    (target / "obsolete.txt").write_text("old generated output")
    codegen.install(output, target)
    assert not (target / "obsolete.txt").exists()
    assert "autogen_probe_value" in (target / "apis/default_api.rs").read_text()
