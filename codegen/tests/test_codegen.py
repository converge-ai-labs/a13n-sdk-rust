"""Generator adapters and generated binding behavior."""

import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "codegen"))
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
    for name in ["Part", "ConnectionConfig"]:
        assert schemas[name] == document["components"]["schemas"][name]
    assert schemas["RunStatus"]["x-rust-unknown-enum"]
    assert "x-rust-unknown-enum" not in schemas["ItemKind"]


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
        {"name": "autogen_probe_value", "in": "query", "schema": {"type": "string"}},
        {
            "name": "flavor",
            "in": "query",
            "schema": {"anyOf": [{"type": "string", "enum": ["warm", "ice_cold"]}, {"type": "null"}]},
        },
    ]
    document["paths"]["/api/v1/autogen-probe"]["get"]["summary"] = "Read a future probe"
    output = codegen.generate(document, tmp_path)
    target = tmp_path / "installed"
    target.mkdir()
    (target / "obsolete.txt").write_text("old generated output")
    codegen.install(output, target)
    assert not (target / "obsolete.txt").exists()
    assert "autogen_probe_value" in (target / "apis/default_api.rs").read_text()
    nodes, tests = codegen.generate_resources(document, target)
    codegen.generate_cli(document, nodes, tests, tmp_path / "cli/generated.rs")
    cli = (tmp_path / "cli/generated.rs").read_text()
    assert '"autogen-probe get"' in cli
    assert "client.resources().autogen_probe().get(" in cli
    assert '"/api/v1/autogen-probe"' in cli
    assert (tmp_path / "cli/schemas.json").exists()
    ordinary = (target / "resources.rs").read_text()
    assert "pub autogen_probe_value:Option<String>" in ordinary
    assert "pub fn autogen_probe(" in ordinary
    assert "pub async fn get(" in ordinary
    assert "/// Read a future probe." in ordinary
    assert "GET /api/v1/autogen-probe" in ordinary
    assert "pub flavor:Option<AutogenProbeFlavor>" in ordinary
    assert '#[serde(rename = "ice_cold")] IceCold' in ordinary
    assert "A local resource reference borrowing" in ordinary
    assert "`None` omits this parameter" in ordinary
    assert "client.resources().autogen_probe().get(" in (target / "resource_tests.rs").read_text()


def test_streaming_and_multimime_adapters_preserve_pinned_contract() -> None:
    document = json.loads((ROOT / "contract/openapi.json").read_text())
    adapted = codegen.prepare(document)
    streaming = 0
    multimime = 0
    for path, item in document["paths"].items():
        for method, operation in item.items():
            if method not in {"get", "post", "put", "patch", "delete"}:
                continue
            converted = adapted["paths"][path][method]
            for status, response in operation.get("responses", {}).items():
                if status.startswith("2") and "text/event-stream" in response.get("content", {}):
                    streaming += 1
                    assert converted["responses"][status]["content"]["text/event-stream"]["schema"] == {
                        "type": "string",
                        "format": "binary",
                    }
            content = operation.get("requestBody", {}).get("content", {})
            if len(content) > 1 and all(
                media.get("schema", {}).get("format") == "binary" for media in content.values()
            ):
                multimime += 1
                parameter = next(p for p in converted["parameters"] if p["name"] == "Content-Type")
                assert parameter["in"] == "header"
                assert parameter["required"] is True
                assert parameter["schema"]["enum"] == list(content)
    assert streaming == 1
    assert multimime == 4
    assert document == json.loads((ROOT / "contract/openapi.json").read_text())


def test_snapshot_stream_and_native_settings_contract_is_preserved() -> None:
    document = json.loads((ROOT / "contract/openapi.json").read_text())
    schemas = document["components"]["schemas"]
    adapted = codegen.prepare(document)["components"]["schemas"]
    for name in ["ModelConfig-Input", "ModelConfig-Output"]:
        assert schemas[name]["properties"]["settings"]["additionalProperties"] == {
            "$ref": "#/components/schemas/JsonValue"
        }
        assert adapted[name]["properties"]["settings"] == schemas[name]["properties"]["settings"]
    assert "resume_after" not in schemas["RunItems"]["required"]
    assert schemas["RunItems"]["properties"]["resume_after"]["anyOf"] == [{"type": "string"}, {"type": "null"}]
    for name in ["AssetCreate", "UploadSource"]:
        assert schemas[name]["properties"]["upload_id"]["pattern"] == r"^upl_[a-f0-9]{32}$"
    for name in ["BootstrapInput", "PasswordChange", "PasswordResetConfirm"]:
        assert schemas[name]["properties"]["password"]["minLength"] == 8
    for name in ["LoginInput", "InvitationAccept"]:
        assert schemas[name]["properties"]["password"]["minLength"] == 1
    parameters = document["paths"]["/api/v1/threads/{thread_id}/stream"]["get"]["parameters"]
    assert [(p["name"], p["in"]) for p in parameters if p["name"] in {"run", "position", "Last-Event-ID"}] == [
        ("run", "query"),
        ("position", "query"),
        ("Last-Event-ID", "header"),
    ]
