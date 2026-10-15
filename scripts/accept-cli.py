"""Exercise a copied CLI binary against an explicitly configured disposable Service.

This optional acceptance suite creates and updates fixture resources. It uses only
CLI subprocesses, not the SDK or direct HTTP calls. Supply A13N_SERVICE_URL,
A13N_API_TOKEN, A13N_WORKSPACE, A13N_AGENT and A13N_CA_BUNDLE from a disposable
HTTPS fixture. Build the CLI first or pass --binary. No registry is contacted.
"""

import argparse
import hashlib
import json
import os
import shutil
import signal
import subprocess
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]


def required(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"The disposable fixture must provide {name}")
    return value


def payload(text: str) -> dict[str, Any]:
    return {"content": [{"type": "text", "text": text}]}


def session_snapshot_ready(frames: list[dict[str, Any]], thread: str, run: str) -> bool:
    """Initial Items follow a current Run, not the last already sealed Run."""
    if not frames:
        return False
    assert frames[0]["type"] == "thread" and frames[0]["data"]["thread"]["id"] == thread
    state = frames[0]["data"]
    if state["current_run"] is None:
        assert state["last_run"] is not None and state["last_run"]["id"] == run
        return True
    assert state["current_run"]["id"] == run
    return any(
        frame["type"] == "items" and frame["data"]["thread_id"] == thread and frame["data"]["run_id"] == run
        for frame in frames[1:]
    )


def offline(source: Path) -> None:
    """Exercise the copied executable against a bounded local TCP origin only."""
    import socket
    import threading

    captured: list[tuple[str, str, Any]] = []
    native_item = {
        "id": "i",
        "kind": "observation",
        "state": "failed",
        "ordinal": 42,
        "response_group": None,
        "failure": {"custom": [False, None, []]},
        "memory_sources": [
            {
                "kind": "record",
                "memory": "notes",
                "status": "recalled",
                "records": [{"id": "r", "sha256": "digest"}],
                "sources_truncated": False,
            }
        ],
    }
    native_event = {
        "type": "TOOL_CALL_RESULT",
        "subagentRunId": "child",
        "content": [
            {"type": "text", "text": "child"},
            {
                "type": "binary",
                "mimeType": "video/mp4",
                "url": "https://media.example/clip.mp4",
                "source": {"kind": "file", "path": "clip.mp4"},
            },
        ],
        "future": None,
    }
    with (
        tempfile.TemporaryDirectory(prefix="a13n-cli-offline-") as name,
        socket.create_server(("127.0.0.1", 0)) as listener,
    ):
        listener.settimeout(10)
        directory = Path(name)
        binary = directory / source.name
        shutil.copy2(source, binary)
        assert binary.read_bytes() == source.read_bytes()
        base = f"http://127.0.0.1:{listener.getsockname()[1]}"
        failures: list[Exception] = []
        schema_result = subprocess.run(
            [str(binary), "models", "create", "--schema"],
            capture_output=True,
            check=True,
            timeout=8,
        )
        schemas = json.loads(schema_result.stdout)["components"]["schemas"]

        def sample(schema: dict[str, Any]) -> Any:
            if "$ref" in schema:
                return sample(schemas[schema["$ref"].rsplit("/", 1)[1]])
            if "const" in schema:
                return schema["const"]
            if "enum" in schema:
                return schema["enum"][0]
            for union in ("anyOf", "oneOf"):
                if union in schema:
                    return sample(next(part for part in schema[union] if part.get("type") != "null"))
            kind = schema.get("type")
            if kind == "object":
                return {key: sample(schema["properties"][key]) for key in schema.get("required", [])}
            if kind == "array":
                return []
            if kind in ("integer", "number"):
                return 1
            if kind == "boolean":
                return True
            if kind == "null":
                return None
            return "2026-10-06T00:00:00Z" if schema.get("format") == "date-time" else "native"

        window = sample(schemas["RunItems"])
        window["complete"] = True
        window["run"]["id"] = "r"
        window["items"] = []
        for ordinal in range(6, 10):
            item = sample(schemas["Item"])
            item.update({"id": f"i{ordinal}", "ordinal": ordinal})
            window["items"].append(item)

        frozen_mounts = [
            {"name": "legacy", "memory_id": "m", "access": "read", "recall": False},
            {
                "name": "shared",
                "memory_id": "m",
                "access": "write",
                "roots": ["global/", "sessions/s/"],
                "space_id": "space",
            },
        ]
        png = bytes([137, 80, 78, 71, 13, 10, 26, 10, 0, 255, 128])

        def serve() -> None:
            try:
                for _ in range(48):
                    connection, _ = listener.accept()
                    with connection:
                        connection.settimeout(5)
                        data = b""
                        while b"\r\n\r\n" not in data:
                            part = connection.recv(4096)
                            assert part, "Incomplete offline request"
                            data += part
                        headers, body = data.split(b"\r\n\r\n", 1)
                        length = next(
                            (
                                int(line.split(b":", 1)[1])
                                for line in headers.split(b"\r\n")
                                if line.lower().startswith(b"content-length:")
                            ),
                            0,
                        )
                        while len(body) < length:
                            part = connection.recv(4096)
                            assert part, "Incomplete offline body"
                            body += part
                        method, path, _ = headers.split(b"\r\n", 1)[0].decode().split()
                        captured.append((method, path, json.loads(body) if body else None))
                        status = 409
                        content_type = "application/json"
                        response: Any = {
                            "error": {
                                "code": "conflict",
                                "message": "capture",
                                "details": {"reason": "run_configuration_immutable"},
                                "request_id": None,
                            }
                        }
                        if path.startswith("/api/v1/model-providers/"):
                            status = 200
                            response = {"provider_id": "p", "state": "disconnected"}
                            if path.endswith("/authorize"):
                                response = {
                                    "attempt_id": "attempt",
                                    "method": "manual_callback",
                                    "authorization_url": "https://provider.example/authorize",
                                    "expires_at": "2026-10-01T12:00:00Z",
                                }
                            elif path.endswith("/models"):
                                response = [{"slug": "native", "display_name": "Native"}]
                            elif method == "DELETE":
                                response = {"local_tokens_cleared": True, "revocation_confirmed": None}
                        if path.startswith("/api/v1/runs/r/items?"):
                            from urllib.parse import parse_qs, urlsplit

                            query = parse_qs(urlsplit(path).query)
                            status = 200
                            response = json.loads(json.dumps(window))
                            if "before" in query or "after" in query:
                                response["items"] = response["items"][:2]
                            if "before" in query and "after" in query:
                                status = 400
                                response = {"error": {"code": "invalid_argument", "message": "exclude before/after"}}
                        if path == "/api/v1/runs/r/contents/c":
                            status = 200
                            response = {
                                "id": "c",
                                "media_type": "application/json",
                                "value": {"z": False, "a": [None, 0]},
                                "truncated": False,
                            }
                        if path == "/api/v1/sessions/s/message-authors?entry_id=e&entry_id=other":
                            status = 200
                            response = {
                                "items": [
                                    {
                                        "entry_id": "e",
                                        "principal_id": "p",
                                        "principal": None,
                                        "submitted_at": "2026-10-13T00:00:00Z",
                                    }
                                ]
                            }
                        if path == "/api/v1/memory-spaces":
                            status, response = 201, sample(schemas["MemorySpace"])
                            response.update({"id": "space", "enabled": False, "always_load": []})
                        if path == "/api/v1/memory-spaces/space/organizations":
                            status, response = 202, {"pending": True, "processed_seq": 0, "history_gap": False}
                        if path == "/api/v1/sessions/s":
                            status, response = 200, sample(schemas["SessionView"])
                            response["memory"] = {"enabled": False, "spaces": []}
                            response["labels"] = {}
                            if method == "PATCH":
                                assert b'if-match: "session:2"' in headers.lower()
                        if path == "/api/v1/runs/r":
                            status, response = 200, sample(schemas["RunView"])
                            response["memory_mounts"] = frozen_mounts
                        if path == "/api/v1/runs/r/agent":
                            status, response = 200, sample(schemas["RunAgent"])
                        if path == "/api/v1/runs/r/attempts/attempt":
                            status, response = 200, sample(schemas["AttemptView"])
                        if path == "/api/v1/usage/records/record/metrics":
                            status, response = (
                                200,
                                {
                                    "time_to_first_chunk_seconds": None,
                                    "request_duration_seconds": 0.0,
                                    "output_tokens_per_second": None,
                                },
                            )
                        if path == "/api/v1/environments/env%20%2F/computer/check":
                            status, response = (
                                200,
                                {
                                    "status": "not_ready",
                                    "display_count": 0,
                                    "observe_ready": False,
                                    "input_ready": False,
                                    "operations": [],
                                },
                            )
                        if path == "/api/v1/runs/r/images/image%20%2F":
                            status, content_type, response = 200, "image/png", png
                        if path == "/api/v1/threads/t/environments/work":
                            assert b'if-match: "thread:2"' in headers.lower()
                            status, response = 200, sample(schemas["MountView"])
                            response["permission_ceiling"] = {"operations": []}
                        if path.endswith("/stream"):
                            status = 200
                            content_type = "text/event-stream"
                            response = (
                                "event: delta\nid: 100-1\ndata: "
                                + json.dumps(
                                    {
                                        "run_id": "root",
                                        "attempt": 1,
                                        "sequence": 1,
                                        "event": native_event,
                                        "item": native_item,
                                    }
                                )
                                + "\n\n"
                            )
                        if path == "/api/v1/sessions/s/stream":
                            assert b"last-event-id:" not in headers.lower()
                            state = {
                                "thread": sample(schemas["ThreadView"]),
                                "current_run": None,
                                "last_run": None,
                                "entries": [],
                            }
                            frames = [
                                ("thread", state),
                                (
                                    "items",
                                    {
                                        "thread_id": "t",
                                        "run_id": "r",
                                        "first": 6,
                                        "items": [],
                                        "continuation": None,
                                        "complete": True,
                                    },
                                ),
                                (
                                    "delta",
                                    {
                                        "thread_id": "t",
                                        "run_id": "r",
                                        "attempt": 1,
                                        "sequence": 2,
                                        "event": native_event,
                                        "item": native_item,
                                    },
                                ),
                                ("gap", {"thread_id": "t", "run_id": "r"}),
                            ]
                            response = "event: future\ndata: not-json\n\n" + "".join(
                                f"event: {kind}\ndata: {json.dumps(value)}\n\n" for kind, value in frames
                            )
                        if path == "/api/v1/sessions/sealed/stream":
                            state = {
                                "thread": sample(schemas["ThreadView"]),
                                "current_run": None,
                                "last_run": sample(schemas["RunView"]),
                                "entries": [],
                            }
                            state["thread"]["id"] = "t"
                            state["last_run"].update({"id": "r", "status": "completed"})
                            response = f"event: thread\ndata: {json.dumps(state)}\n\n"
                        encoded = (
                            response
                            if isinstance(response, bytes)
                            else response.encode()
                            if isinstance(response, str)
                            else json.dumps(response).encode()
                        )
                        connection.sendall(
                            f"HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\nContent-Length: {len(encoded)}\r\nConnection: close\r\n\r\n".encode()
                            + encoded
                        )
            except Exception as error:
                failures.append(error)

        server = threading.Thread(target=serve)
        server.start()
        environment = {key: value for key, value in os.environ.items() if not key.startswith("A13N_")}
        environment.update({"HOME": str(directory), "USERPROFILE": str(directory), "XDG_CONFIG_HOME": str(directory)})

        def execute(*arguments: str, stdin: bytes | None = None, code: int = 0, jsonl: bool = False) -> Any:
            result = subprocess.run(
                [str(binary), "--base-url", base, "--timeout", "5", *arguments],
                cwd=directory,
                env=environment,
                input=stdin,
                capture_output=True,
                timeout=8,
            )
            assert result.returncode == code, "Copied CLI offline outcome mismatch"
            if jsonl:
                return [json.loads(line) for line in result.stdout.splitlines()]
            return json.loads(result.stdout) if result.stdout else None

        configurations = [
            {},
            {"configuration": None},
            {"configuration": {}},
            {"configuration": {"allowed_hosts": None}},
            {"configuration": {"allowed_hosts": []}},
            {
                "configuration": {
                    "extensions": {
                        "example.native": {"enabled": False, "zero": 0, "empty": [], "nested": {"value": None}}
                    }
                }
            },
        ]
        native_payload = {
            "content": [
                {"type": "text", "text": ""},
                {"type": "url", "url": "https://media.example/image.png"},
                {"type": "asset", "asset_id": "asset_native"},
            ]
        }
        file = directory / "options.json"
        for options in configurations:
            file.write_text(json.dumps(options), encoding="utf-8")
            execute(
                "agents",
                "start",
                "a",
                "--payload",
                json.dumps(native_payload),
                "--options",
                f"@{file}",
                "--idempotency-key",
                "offline-key",
                code=4,
            )
            execute(
                "agents",
                "send",
                "a",
                "--thread",
                "t",
                "--payload",
                "@-",
                "--options",
                json.dumps(options),
                "--idempotency-key",
                "offline-key",
                stdin=json.dumps(native_payload).encode(),
                code=4,
            )
            execute(
                "threads",
                "create",
                "--body",
                json.dumps({"agent_id": "a", "payload": native_payload, "options": options}),
                "--idempotency-key",
                "offline-key",
                code=4,
            )
            assert all(
                request[2]["options"] == options and request[2]["payload"] == native_payload
                for request in captured[-3:]
            ), "Copied CLI lost native configuration/payload"
        execute("model-providers", "authorization", "get", "--provider", "p")
        execute("model-providers", "authorize", "p", "--body", '{"new_registration":false}')
        execute(
            "model-providers",
            "authorization",
            "callback",
            "p",
            "--body",
            '{"attempt_id":"attempt","callback_url":"https://callback.example/?code=offline"}',
        )
        disconnected = execute("model-providers", "authorization", "delete", "--provider", "p")
        assert disconnected["revocation_confirmed"] is None
        assert execute("model-providers", "models", "get", "--provider", "p")[0]["slug"] == "native"
        characteristics = {"image_input": {"split_large_images": False, "max_image_bytes": 0}, "video_input": {}}
        execute(
            "models",
            "create",
            "--body",
            json.dumps(
                {
                    "name": "native",
                    "provider_id": "p",
                    "config": {
                        "model_api": "provider:model",
                        "model_name": "native",
                        "characteristics": characteristics,
                    },
                }
            ),
            code=4,
        )
        assert captured[-1][2]["config"]["characteristics"] == characteristics
        frame = execute("threads", "events", "--thread", "t")
        assert frame["event"] == native_event and frame["item"] == native_item
        session_frames = execute("sessions", "events", "s", jsonl=True)
        assert [frame["type"] for frame in session_frames] == ["thread", "items", "delta", "gap"]
        assert session_frames[2]["data"]["event"] == native_event and session_frames[2]["data"]["item"] == native_item
        assert all("cursor" not in frame for frame in session_frames)
        raw = directory / "session.sse"
        execute("sessions", "stream", "get", "--session", "s", "--output", str(raw))
        assert raw.read_text().startswith("event: future\ndata: not-json\n\n") and "event: gap" in raw.read_text()
        sealed_frames = execute("sessions", "events", "sealed", jsonl=True)
        assert len(sealed_frames) == 1 and session_snapshot_ready(sealed_frames, "t", "r")
        active_frames = json.loads(json.dumps(sealed_frames))
        active_frames[0]["data"]["current_run"] = active_frames[0]["data"]["last_run"]
        assert not session_snapshot_ready(active_frames, "t", "r")
        assert session_snapshot_ready([*active_frames, session_frames[1]], "t", "r")
        # A sealed-only fresh snapshot uses explicit historical Run Items readback.
        recent = execute("runs", "items", "get", "--run", "r", "--limit", "2")
        assert recent == window and len(recent["items"]) == 4 and recent["items"][0]["ordinal"] == 6
        for flag, value in (("--before", "8"), ("--after", "0")):
            history = execute("runs", "items", "get", "--run", "r", flag, value, "--limit", "2")
            assert history["complete"] and len(history["items"]) == 2
            assert set(history) == {"run", "items", "complete"}
            assert captured[-1][1] == f"/api/v1/runs/r/items?{flag[2:]}={value}&limit=2"
        execute("runs", "items", "get", "--run", "r", "--before", "1", "--after", "0", code=2)
        content = execute("runs", "contents", "get", "c", "--run", "r")
        assert content["value"] == {"z": False, "a": [None, 0]} and not content["truncated"]
        authors = execute(
            "sessions", "message-authors", "list", "--session", "s", "--entry-id", "e", "--entry-id", "other"
        )
        assert authors["items"][0]["entry_id"] == "e"
        assert captured[-1][1] == "/api/v1/sessions/s/message-authors?entry_id=e&entry_id=other"
        ordered_history = [
            {
                "parts": [{"args": {"z": 0, "a": {"second": 2, "first": 1}}, "part_kind": "tool-call"}],
                "kind": "response",
            }
        ]
        execute(
            "threads",
            "create",
            "--body",
            json.dumps(
                {
                    "agent_id": "a",
                    "payload": native_payload,
                    "message_history": ordered_history,
                    "options": {
                        "overrides": {"lazy_environment": False, "skill_sync": {"mode": "isolated", "cache_count": 0}}
                    },
                }
            ),
            "--idempotency-key",
            "ordered",
            code=4,
        )
        assert json.dumps(captured[-1][2]["message_history"]) == json.dumps(ordered_history)
        assert captured[-1][2]["options"]["overrides"] == {
            "lazy_environment": False,
            "skill_sync": {"mode": "isolated", "cache_count": 0},
        }
        native_selection = {
            "overrides": {
                "builtin_skills": [],
                "platform_tools": {"groups": [], "read_permission": "deny", "write_permission": "ask"},
            }
        }
        execute(
            "agents",
            "start",
            "a",
            "--text",
            "selected",
            "--options",
            json.dumps(native_selection),
            "--idempotency-key",
            "selected",
            code=4,
        )
        assert captured[-1][2]["options"] == native_selection
        space_body = {
            "name": "shared",
            "enabled": False,
            "always_load": [],
            "organization": {"enabled": False, "cooldown_seconds": 0, "trigger": {"type": "any_change"}},
        }
        assert not execute("memory-spaces", "create", "--body", json.dumps(space_body))["enabled"]
        assert captured[-1][2] == space_body
        assert execute("memory-spaces", "organizations", "create", "--space", "space")["pending"]
        session_body = {"memory": {"enabled": False, "spaces": []}}
        execute("sessions", "update", "s", "--if-match", '"session:2"', "--body", json.dumps(session_body))
        assert captured[-1][2] == session_body and "labels" not in captured[-1][2]
        execute("labels", "session", "s")
        execute("labels", "session", "s", "--if-match", '"session:2"', "--set", "{}")
        assert captured[-1][2] == {"labels": {}}
        assert execute("runs", "get", "r")["memory_mounts"] == frozen_mounts
        execute("runs", "agent", "get", "--run", "r")
        execute("runs", "attempts", "get", "attempt", "--run", "r")
        metrics = execute("usage", "records", "metrics", "get", "--record", "record")
        assert metrics == {
            "time_to_first_chunk_seconds": None,
            "request_duration_seconds": 0.0,
            "output_tokens_per_second": None,
        }
        assert execute("environments", "computer", "check", "env /")["status"] == "not_ready"
        image = directory / "retained.png"
        execute("runs", "images", "get", "image /", "--run", "r", "--output", str(image))
        assert image.read_bytes() == png
        execute(
            "threads",
            "environments",
            "update",
            "work",
            "--thread",
            "t",
            "--if-match",
            '"thread:2"',
            "--body",
            '{"permission_ceiling":{"operations":[]}}',
        )
        assert captured[-1][2] == {"permission_ceiling": {"operations": []}}
        server.join(timeout=12)
        assert not server.is_alive() and not failures and len(captured) == 48, (
            "Copied CLI offline server did not complete"
        )
    print(
        "Copied CLI local TCP: configuration omission/null/empty/native JSON, typed payload file/stdin, media policy, five OAuth/discovery commands, native child ItemRef/memory provenance, ordered imported history, explicit Run content/Session authors, skill/lazy overrides, ordinal windows, whole tail > limit, native Session frames/raw bytes and bounded historical windows passed",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        default=ROOT
        / "a13n-service-cli/target/debug"
        / ("a13n-service-cli.exe" if os.name == "nt" else "a13n-service-cli"),
    )
    parser.add_argument(
        "--offline", action="store_true", help="Copied-binary local TCP checks; no Service or provider calls"
    )
    args = parser.parse_args()
    source: Path = args.binary.resolve()
    if not source.is_file():
        raise SystemExit("Build the CLI with make cli-build or supply --binary")
    if args.offline:
        offline(source)
        return
    service = required("A13N_SERVICE_URL")
    if not service.startswith("https://"):
        raise SystemExit("Acceptance requires an HTTPS Service and its trusted CA")
    token = required("A13N_API_TOKEN")
    workspace = required("A13N_WORKSPACE")
    agent = required("A13N_AGENT")
    client_tool_agent = required("A13N_CLIENT_TOOL_AGENT")
    organization = required("A13N_ORGANIZATION")
    ca = required("A13N_CA_BUNDLE")
    with tempfile.TemporaryDirectory(prefix="a13n-cli-consumer-") as name:
        directory = Path(name)
        binary = directory / source.name
        shutil.copy2(source, binary)
        assert hashlib.sha256(binary.read_bytes()).digest() == hashlib.sha256(source.read_bytes()).digest()
        config = directory / "config.json"
        config.write_text(
            json.dumps(
                {
                    "profiles": {
                        "acceptance": {
                            "base_url": service,
                            "workspace": workspace,
                            "ca_bundle": ca,
                            "token_env": "CLI_ACCEPTANCE_TOKEN",
                        }
                    }
                }
            ),
            encoding="utf-8",
        )
        environment = {key: value for key, value in os.environ.items() if not key.startswith("A13N_")}
        environment.update(
            {
                "HOME": str(directory),
                "USERPROFILE": str(directory),
                "XDG_CONFIG_HOME": str(directory),
                "A13N_CLI_CONFIG": str(config),
                "CLI_ACCEPTANCE_TOKEN": token,
                # An explicit profile must not inherit another environment's target.
                "A13N_BASE_URL": "https://invalid.invalid",
                "A13N_WORKSPACE": "wrong-workspace",
            }
        )

        def execute(
            *arguments: str,
            stdin: bytes | None = None,
            success: bool = True,
            credential: bool = True,
            timeout: int = 90,
        ) -> subprocess.CompletedProcess[bytes]:
            process_environment = environment.copy()
            if not credential:
                process_environment.pop("CLI_ACCEPTANCE_TOKEN")
            result = subprocess.run(
                [str(binary), "--profile", "acceptance", "--timeout", str(timeout), *arguments],
                cwd=directory,
                env=process_environment,
                input=stdin,
                capture_output=True,
                timeout=100,
            )
            # Never print credentials or request bodies while reporting failures.
            assert token.encode() not in result.stdout + result.stderr, "CLI exposed its fixture credential"
            if (result.returncode == 0) != success:
                raise AssertionError(
                    f"CLI outcome mismatch for {arguments[0] if arguments else 'root'}: exit {result.returncode}"
                )
            return result

        def call(*arguments: str, body: Any = None, status: int | None = None) -> dict[str, Any]:
            command = ["--include-meta", *arguments]
            if body is not None:
                command.extend(["--body", "@-"])
            result = execute(*command, stdin=json.dumps(body).encode() if body is not None else None)
            value = json.loads(result.stdout)
            assert isinstance(value, dict), "Expected a response envelope"
            if status is not None:
                assert value["status"] == status, "Unexpected Service HTTP status"
            assert "data" in value, "Response metadata mode lost the API data"
            return value

        def read_etag(value: dict[str, Any]) -> str:
            etag = value["etag"]
            assert isinstance(etag, str) and etag, "Read response must expose its actual ETag"
            return etag

        # The copied executable discovers its commands without a source checkout.
        assert b"agents" in execute("--help", credential=False).stdout
        assert b"COMPREPLY" in execute("completion", "bash", credential=False).stdout
        plan = json.loads(
            execute(
                "--base-url",
                "https://invalid.invalid",
                "--dry-run",
                "agents",
                "get",
                agent,
                credential=False,
            ).stdout
        )
        assert plan["local_only"] is True and plan["method"] == "GET"
        invalid = execute("--error-format", "json", "agents", "get", success=False, credential=False)
        assert invalid.returncode == 2 and not invalid.stdout
        assert json.loads(invalid.stderr)["error"]["kind"] == "input"
        schema = json.loads(execute("agents", "create", "--schema", credential=False).stdout)
        assert isinstance(schema, dict), "Request schema must be machine-readable"
        shown = execute("config", "show").stdout
        assert service.encode() in shown and b"wrong-workspace" not in shown
        assert json.loads(execute("--include-meta", "healthz", "get", credential=False).stdout)["status"] == 200
        assert (
            json.loads(execute("--include-meta", "auth", "configuration", "get", credential=False).stdout)["status"]
            == 200
        )
        no_auth = execute("--error-format", "json", "agents", "get", agent, success=False, credential=False)
        assert no_auth.returncode == 3 and not no_auth.stdout
        assert json.loads(no_auth.stderr)["error"]["status"] == 401
        call("readyz", "get", status=200)
        original = call("agents", "get", agent, status=200)
        initial_etag = read_etag(original)
        description = f"CLI acceptance {uuid.uuid4()}"
        changed = call(
            "agents", "update", agent, "--if-match", initial_etag, body={"description": description}, status=200
        )
        assert read_etag(changed) != initial_etag
        cleared = call(
            "agents", "update", agent, "--if-match", read_etag(changed), body={"description": None}, status=200
        )
        # Agent metadata explicitly treats null as keep-current, not clear-to-null.
        # Wire preservation is covered by local CLI fixtures; live behavior follows Service.
        assert cleared["data"]["description"] == description, "Null changed a keep-current metadata field"
        assert cleared["data"]["name"] == original["data"]["name"], "Omitted field changed"
        conflict = execute(
            "--error-format",
            "json",
            "agents",
            "update",
            agent,
            "--if-match",
            initial_etag,
            "--body",
            "{}",
            success=False,
        )
        assert not conflict.stdout, "Failures must not contaminate stdout"
        error = json.loads(conflict.stderr)
        assert conflict.returncode == 4 and error["error"]["status"] == 412
        assert error["error"]["code"] and error["error"]["request_id"], "API error lost its structured evidence"
        print(
            "Copied CLI verified HTTPS: offline UX, public/no-auth, profile, metadata, nullable PATCH and stale CAS",
            flush=True,
        )

        # This journey uses the copied CLI only; parent supplies the matching Service.
        created_space = call(
            "memory-spaces",
            "create",
            body={
                "name": "CLI Space",
                "always_load": [],
                "organization": {
                    "enabled": False,
                    "cooldown_seconds": 0,
                    "trigger": {"type": "change_count", "count": 1},
                },
            },
            status=201,
        )
        space_id = created_space["data"]["id"]
        current_space = call("memory-spaces", "get", space_id)
        call(
            "memory-spaces",
            "update",
            space_id,
            "--if-match",
            read_etag(current_space),
            body={"description": None, "guide": None},
        )
        assert any(item["id"] == space_id for item in call("memory-spaces", "list")["data"]["items"])
        organization_status = call("memory-spaces", "organization", "get", "--space", space_id)["data"]
        assert not organization_status["pending"] and organization_status["active_run_id"] is None
        assert call("memory-spaces", "organizations", "list", "--space", space_id)["data"]["items"] == []
        selected = {"enabled": True, "spaces": [{"name": "shared", "space_id": space_id}]}
        selected_session = call("sessions", "create", body={"memory": selected}, status=201)
        selected_id = selected_session["data"]["id"]
        call("labels", "session", selected_id, "--set", "{}", "--if-match", read_etag(selected_session))
        read_session = call("sessions", "get", selected_id)
        assert (
            read_session["data"]["memory"]["enabled"]
            and read_session["data"]["memory"]["spaces"][0]["space_id"] == space_id
        )
        selected_receipt = call(
            "threads",
            "create",
            "--idempotency-key",
            str(uuid.uuid4()),
            body={
                "agent_id": agent,
                "session_id": selected_id,
                "payload": payload("CLI shared Space readback"),
                "options": {"overrides": {"builtin_skills": [], "platform_tools": {"groups": []}}},
            },
            status=201,
        )["data"]
        incorporated = call(
            "threads", "inbox", "wait", selected_receipt["entry"]["id"], "--thread", selected_receipt["thread"]["id"]
        )["data"]
        assert incorporated["status"] == "consumed"
        selected_run = incorporated["assigned_run_id"]
        assert call("runs", "wait", selected_run)["data"]["status"] == "completed"
        frozen = call("runs", "get", selected_run)["data"]["memory_mounts"]
        shared = next(mount for mount in frozen if mount.get("space_id") == space_id)
        assert shared["roots"] == ["global/", f"sessions/{selected_id}/"]
        executed = call("runs", "agent", "get", "--run", selected_run)["data"]
        assert executed["config"]["builtin_skills"] == [] and executed["config"]["platform_tools"]["groups"] == []
        assert any(
            run["id"] == selected_run for run in call("runs", "list", "--session-id", selected_id)["data"]["items"]
        )
        attempt = call("runs", "attempts", "list", "--run", selected_run)["data"]["items"][0]
        assert call("runs", "attempts", "get", attempt["id"], "--run", selected_run)["data"]["run_id"] == selected_run
        summary = call("usage", "get", "--run-id", selected_run)["data"]
        assert all(run["run_id"] == selected_run for run in summary["runs"])
        assert any(
            session["id"] == selected_id
            for session in call("memory-spaces", "sessions", "list", "--space", space_id)["data"]["items"]
        )
        current_session = call("sessions", "get", selected_id)
        call(
            "sessions",
            "update",
            selected_id,
            "--if-match",
            read_etag(current_session),
            body={"memory": {"enabled": False, "spaces": []}},
        )
        assert call("runs", "get", selected_run)["data"]["memory_mounts"] == frozen
        current_space = call("memory-spaces", "get", space_id)
        call("memory-spaces", "delete", space_id, "--if-match", read_etag(current_space), status=204)
        if organizer_model := os.environ.get("A13N_ORGANIZER_MODEL"):
            organizer = call(
                "memory-spaces",
                "create",
                body={"name": "CLI organization request", "organization": {"enabled": False, "model": organizer_model}},
                status=201,
            )
            call("memory-spaces", "organizations", "create", "--space", organizer["data"]["id"], status=202)
            print(
                "Copied CLI verified HTTPS: manual organization request accepted 202; execution not awaited and disposable Space retained for fixture cleanup",
                flush=True,
            )
        else:
            print(
                "NOT_RUN HTTPS manual organization request: A13N_ORGANIZER_MODEL not supplied; 202 wire covered offline",
                flush=True,
            )
        if record := os.environ.get("A13N_MODEL_RECORD"):
            call("usage", "records", "metrics", "get", "--record", record)
            print("Copied CLI verified HTTPS: supplied model record metrics", flush=True)
        else:
            print(
                "NOT_RUN HTTPS single-record metrics: A13N_MODEL_RECORD not supplied; nullable/zero covered offline",
                flush=True,
            )
        print(
            "Copied CLI verified HTTPS: Space CRUD/CAS/status/history, labels preserve Session memory, frozen roots, Run search/executed Agent/Attempt and usage; Computer/provider/screenshots remain offline-only",
            flush=True,
        )

        memory = call("memories", "create", body={"name": "CLI acceptance"}, status=201)["data"]
        memory_id = memory["id"]
        path = "projects/计划 #1%.md"
        created = call(
            "memories", "files", "create", "--memory", memory_id, body={"path": path, "content": "first"}, status=201
        )
        file_args = ["--memory", memory_id, "--path", path]
        assert call("memories", "files", "get", *file_args)["data"]["content"] == "first"
        updated = call(
            "memories", "files", "replace", *file_args, "--if-match", read_etag(created), body={"content": "second"}
        )
        pages = json.loads(
            execute(
                "--include-meta",
                "memories",
                "revisions",
                "list",
                "--memory",
                memory_id,
                "--path",
                path,
                "--limit",
                "1",
                "--all",
            ).stdout
        )
        revisions = [item for page in pages["pages"] for item in page["data"]["items"]]
        assert len(revisions) == 2 and len(pages["pages"]) == 2, "All-pages mode did not preserve both pages"
        create_seq = next(item["seq"] for item in revisions if item["op"] == "create")
        update_seq = next(item["seq"] for item in revisions if item["op"] == "update")
        assert isinstance(create_seq, int) and isinstance(update_seq, int)
        call("memories", "files", "delete", *file_args, "--if-match", read_etag(updated))
        restored = call("memories", "revisions", "restore", str(update_seq), "--memory", memory_id)
        assert restored["data"]["file"]["content"] == "first"
        current_file = call("memories", "files", "get", *file_args)
        undone = call(
            "memories",
            "revisions",
            "restore",
            str(create_seq),
            "--memory",
            memory_id,
            "--if-match",
            read_etag(current_file),
        )
        assert undone["data"]["file"] is None, "Nullable restore response lost its explicit null"
        print(
            "Copied CLI verified HTTPS: Unicode file paths, per-page metadata, numeric revisions and nullable restore",
            flush=True,
        )

        content = b"asset\n" * 50_000
        upload_path = directory / "upload.bin"
        upload_path.write_bytes(content)
        uploaded = call(
            "uploads",
            "create",
            "--file",
            str(upload_path),
            "--content-type",
            "application/octet-stream",
            "--idempotency-key",
            str(uuid.uuid4()),
        )
        asset = call("assets", "create", body={"name": "CLI binary", "upload_id": uploaded["data"]["upload_id"]})[
            "data"
        ]
        output = directory / "download.bin"
        execute("assets", "content", "get", "--asset", asset["id"], "--output", str(output))
        assert output.read_bytes() == content and output.stat().st_size == 300_000
        png = bytes.fromhex(
            "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c63f8cfc0f01f00050001ff89993d1d0000000049454e44ae426082"
        )
        image = directory / "avatar.png"
        image.write_bytes(png)
        current_agent = call("agents", "get", agent)
        call(
            "agents",
            "avatar",
            "replace",
            "--agent",
            agent,
            "--file",
            str(image),
            "--content-type",
            "image/png",
            "--if-match",
            read_etag(current_agent),
        )
        received_image = directory / "received.png"
        execute("agents", "avatar", "get", "--agent", agent, "--output", str(received_image))
        assert received_image.read_bytes() == png
        print("Copied CLI verified HTTPS: 300KB multipart/binary and explicit PNG MIME", flush=True)

        large_text = "Saved display Unicode evidence: " + "語" * 16000
        large = call(
            "threads",
            "create",
            "--body",
            json.dumps(
                {
                    "agent_id": agent,
                    "payload": payload(large_text),
                    "options": {
                        "overrides": {"lazy_environment": False, "skill_sync": {"mode": "isolated", "cache_count": 0}}
                    },
                }
            ),
            "--idempotency-key",
            str(uuid.uuid4()),
            status=201,
        )["data"]
        deadline = time.monotonic() + 80
        while True:
            incorporated = call("threads", "inbox", "get", large["entry"]["id"], "--thread", large["thread"]["id"])[
                "data"
            ]
            if incorporated["status"] == "consumed":
                break
            assert incorporated["status"] in {"pending", "assigned"} and time.monotonic() < deadline
            time.sleep(0.1)
        large_run_id = incorporated["assigned_run_id"]
        assert call("runs", "wait", large_run_id)["data"]["status"] == "completed"
        large_items = call("runs", "items", "get", "--run", large_run_id)["data"]
        reference = next(
            ref
            for item in large_items["items"]
            for ref in item.get("content_refs", {}).values()
            if ref["size_bytes"] == len(large_text.encode())
        )
        content = call("runs", "contents", "get", reference["id"], "--run", large_run_id)["data"]
        assert content["value"] == large_text and content["media_type"] == "text/plain" and not content["truncated"]
        authors = call(
            "sessions",
            "message-authors",
            "list",
            "--session",
            large["thread"]["session_id"],
            "--entry-id",
            large["entry"]["id"],
            "--entry-id",
            "ent_missing",
        )["data"]["items"]
        assert len(authors) == 1 and authors[0]["entry_id"] == large["entry"]["id"]
        print(
            "Copied CLI verified HTTPS: 48KB Unicode content refs/read, exact Entry authors, lazy/isolated options",
            flush=True,
        )
        history = [
            {"parts": [{"content": "Imported question.", "part_kind": "user-prompt"}], "kind": "request"},
            {
                "parts": [
                    {
                        "args": {"z": 0, "a": {"second": 2, "first": 1}},
                        "tool_call_id": "imported",
                        "tool_name": "lookup",
                        "part_kind": "tool-call",
                    }
                ],
                "kind": "response",
            },
            {
                "parts": [
                    {
                        "content": {"z": [], "a": False},
                        "tool_call_id": "imported",
                        "tool_name": "lookup",
                        "part_kind": "tool-return",
                    }
                ],
                "kind": "request",
            },
        ]
        request = {
            "agent_id": agent,
            "payload": payload("[slow] [long] CLI installed-binary acceptance."),
            "message_history": history,
        }
        request_file = directory / "thread.json"
        request_file.write_text(json.dumps(request), encoding="utf-8")
        request_key = str(uuid.uuid4())
        first = call("threads", "create", "--body", f"@{request_file}", "--idempotency-key", request_key, status=201)
        replay = call("threads", "create", "--body", f"@{request_file}", "--idempotency-key", request_key, status=200)
        assert first["data"]["thread"]["id"] == replay["data"]["thread"]["id"]
        assert "message_history" not in first["data"]["thread"]
        run_id = first["data"]["run"]["id"]
        thread_id = first["data"]["thread"]["id"]
        assert "message_history" not in call("threads", "get", thread_id)["data"]
        if os.name != "nt":
            session_id = first["data"]["thread"]["session_id"]
            events_file = directory / "events.jsonl"
            errors_file = directory / "events.stderr"
            with events_file.open("wb") as events_output, errors_file.open("wb") as events_errors:
                observer = subprocess.Popen(
                    [
                        str(binary),
                        "--profile",
                        "acceptance",
                        "sessions",
                        "events",
                        session_id,
                    ],
                    cwd=directory,
                    env=environment,
                    stdout=events_output,
                    stderr=events_errors,
                )
                try:
                    deadline = time.monotonic() + 20
                    while True:
                        complete_lines = events_file.read_bytes().split(b"\n")[:-1]
                        delivered = [json.loads(line) for line in complete_lines if line]
                        if session_snapshot_ready(delivered, thread_id, run_id):
                            break
                        assert observer.poll() is None, "SSE observer exited before delivering Session snapshots"
                        assert time.monotonic() < deadline, "Session did not flush initial snapshots while observing"
                        time.sleep(0.05)
                    observer.send_signal(signal.SIGINT)
                    assert observer.wait(timeout=10) == 130, "Local Ctrl-C must use the documented cancellation exit"
                finally:
                    if observer.poll() is None:
                        observer.kill()
                        observer.wait(timeout=10)
            observed = events_file.read_bytes()
            assert token.encode() not in observed + errors_file.read_bytes(), "SSE exposed its credential"
            frames = [json.loads(line) for line in observed.splitlines()]
            assert frames and all(isinstance(frame, dict) for frame in frames), "Events must be complete JSONL"
            assert session_snapshot_ready(frames, thread_id, run_id)
            assert all("cursor" not in frame for frame in frames)
            reopened = execute("sessions", "events", session_id, success=False, timeout=2)
            assert reopened.returncode == 5
            fresh = [json.loads(line) for line in reopened.stdout.splitlines()]
            assert session_snapshot_ready(fresh, thread_id, run_id)
            for snapshot in (frames, fresh):
                if snapshot[0]["data"]["current_run"] is None:
                    saved = call("runs", "items", "get", "--run", run_id)["data"]
                    assert saved["run"]["id"] == run_id and saved["complete"]
            still_running = call("runs", "get", run_id)["data"]
            assert still_running["status"] != "cancelled", "Stopping observation cancelled the durable Run"
            print(
                "Copied CLI verified direct HTTPS: native Session snapshots, fresh explicit reopen, flushed JSONL and local-only Ctrl-C/timeout",
                flush=True,
            )
        waited = call("runs", "wait", run_id)
        assert waited["data"]["id"] == run_id and waited["data"]["status"] == "completed"
        recent = call("runs", "items", "get", "--run", run_id, "--limit", "1")["data"]
        assert recent["complete"] and set(recent) == {"run", "items", "complete"}
        assert all(
            right["ordinal"] == left["ordinal"] + 1
            for left, right in zip(recent["items"], recent["items"][1:], strict=False)
        )
        historical = call("runs", "items", "get", "--run", run_id, "--after", "0", "--limit", "1")["data"]
        assert historical["complete"] and len(historical["items"]) <= 1
        if historical["items"]:
            assert historical["items"][0]["ordinal"] == 1
        before = call("runs", "items", "get", "--run", run_id, "--before", "1", "--limit", "1")["data"]
        assert not before["items"]
        print("Copied CLI verified HTTPS: native ordinal windows without stream progress", flush=True)
        print(
            "Copied CLI verified HTTPS: native imported history input and current Thread metadata, 201/200 replay and exact Run wait",
            flush=True,
        )
        native_options = {
            "configuration": {
                "allowed_hosts": None,
                "extensions": {
                    "example.acceptance": {"enabled": False, "zero": 0, "empty": [], "nested": {"value": None}}
                },
            }
        }
        options_file = directory / "native-options.json"
        options_file.write_text(json.dumps(native_options), encoding="utf-8")
        native_payload_file = directory / "native-payload.json"
        native_payload_file.write_text(json.dumps(payload("CLI native typed payload acceptance.")), encoding="utf-8")
        configured = json.loads(
            execute(
                "--include-meta",
                "agents",
                "start",
                agent,
                "--payload",
                f"@{native_payload_file}",
                "--options",
                f"@{options_file}",
                "--idempotency-key",
                str(uuid.uuid4()),
                "--wait",
            ).stdout
        )
        assert configured["outcome"]["data"]["status"] == "completed"
        assert configured["outcome"]["data"]["options"]["configuration"] == native_options["configuration"]
        configured_thread = configured["submitted"]["data"]["thread"]["id"]
        configured_followup = json.loads(
            execute(
                "--include-meta",
                "agents",
                "send",
                agent,
                "--thread",
                configured_thread,
                "--payload",
                "@-",
                "--options",
                json.dumps(native_options),
                "--idempotency-key",
                str(uuid.uuid4()),
                "--wait",
                stdin=json.dumps(payload("Native follow-up via stdin.")).encode(),
            ).stdout
        )
        assert configured_followup["outcome"]["data"]["status"] == "completed"
        assert configured_followup["outcome"]["data"]["options"]["configuration"] == native_options["configuration"]
        # Use generated Message delivery for a different next_run snapshot; the helpers do not invent scheduling aliases.
        active = call(
            "threads",
            "create",
            "--idempotency-key",
            str(uuid.uuid4()),
            body={
                "agent_id": agent,
                "payload": payload("[interruptible] CLI frozen configuration."),
                "options": native_options,
            },
        )["data"]
        active_deadline = time.monotonic() + 20
        while True:
            active_state = call("runs", "get", active["run"]["id"])["data"]
            if active_state["status"] == "running":
                break
            assert active_state["status"] == "accepted" and time.monotonic() < active_deadline, (
                "Configuration fixture ended before running conflict test"
            )
            time.sleep(0.05)
        rejected = execute(
            "--error-format",
            "json",
            "threads",
            "inbox",
            "create",
            "--thread",
            active["thread"]["id"],
            "--idempotency-key",
            str(uuid.uuid4()),
            "--body",
            json.dumps(
                {
                    "agent_id": agent,
                    "delivery": "steer",
                    "payload": payload("Conflicting snapshot."),
                    "options": {"configuration": {"allowed_hosts": []}},
                }
            ),
            success=False,
        )
        assert rejected.returncode == 4 and json.loads(rejected.stderr)["error"]["status"] == 409
        queued_native = call(
            "threads",
            "inbox",
            "create",
            "--thread",
            active["thread"]["id"],
            "--idempotency-key",
            str(uuid.uuid4()),
            body={
                "agent_id": agent,
                "delivery": "next_run",
                "payload": payload("Next native snapshot."),
                "options": {"configuration": {"allowed_hosts": None, "extensions": {"example.next": False}}},
            },
        )["data"]
        first_native = call("runs", "wait", active["run"]["id"])["data"]
        assert first_native["options"]["configuration"] == native_options["configuration"]
        incorporated = call(
            "threads", "inbox", "wait", queued_native["entry"]["id"], "--thread", active["thread"]["id"]
        )["data"]
        later_native = call("runs", "wait", incorporated["assigned_run_id"])["data"]
        assert later_native["id"] != active["run"]["id"] and later_native["options"]["configuration"]["extensions"] == {
            "example.next": False
        }
        print(
            "Copied CLI verified HTTPS: typed --payload/--options file/stdin, frozen configuration readback, conflict and next_run",
            flush=True,
        )

        ordinary = json.loads(
            execute(
                "--include-meta",
                "agents",
                "start",
                agent,
                "--text",
                "CLI Agent helper acceptance.",
                "--idempotency-key",
                str(uuid.uuid4()),
                "--wait",
            ).stdout
        )
        assert ordinary["outcome"]["data"]["status"] == "completed"
        ordinary_thread = ordinary["submitted"]["data"]["thread"]["id"]
        continued = json.loads(
            execute(
                "--include-meta",
                "agents",
                "send",
                agent,
                "--thread",
                ordinary_thread,
                "--text",
                "CLI Agent follow-up.",
                "--idempotency-key",
                str(uuid.uuid4()),
                "--wait",
            ).stdout
        )
        assert continued["submitted"]["data"]["thread"]["id"] == ordinary_thread
        assert continued["outcome"]["data"]["status"] == "completed"

        queued_source = call(
            "threads",
            "create",
            "--idempotency-key",
            str(uuid.uuid4()),
            body={"agent_id": agent, "payload": payload("[interruptible] Wait for queued CLI inbox.")},
            status=201,
        )["data"]
        queued = call(
            "threads",
            "inbox",
            "create",
            "--thread",
            queued_source["thread"]["id"],
            "--idempotency-key",
            str(uuid.uuid4()),
            body={"agent_id": agent, "payload": payload("Queued CLI message.")},
        )["data"]
        assert queued["run"] is None, "Queued receipt must not fabricate a Run"
        assert call("runs", "wait", queued_source["run"]["id"])["data"]["status"] == "completed"
        consumed = call("threads", "inbox", "wait", queued["entry"]["id"], "--thread", queued_source["thread"]["id"])[
            "data"
        ]
        assert consumed["status"] == "consumed", "Entry wait ended at assignment, not consumption"
        successor = call("runs", "wait", consumed["assigned_run_id"])["data"]
        assert successor["status"] == "completed"

        def history_followup(thread: str, previous: str) -> None:
            read = call("threads", "get", thread)["data"]
            assert read["current_run_id"] is None and read["last_run_id"] == previous
            next_result = json.loads(
                execute(
                    "--include-meta",
                    "agents",
                    "send",
                    agent,
                    "--thread",
                    thread,
                    "--text",
                    "Continue after the sealed outcome with new evidence.",
                    "--idempotency-key",
                    str(uuid.uuid4()),
                    "--wait",
                ).stdout
            )
            assert next_result["outcome"]["data"]["status"] == "completed"
            assert next_result["outcome"]["data"]["parent_run_id"] == previous

        interrupted = call(
            "threads",
            "create",
            "--idempotency-key",
            str(uuid.uuid4()),
            body={"agent_id": agent, "payload": payload("[interruptible] Interrupt CLI run.")},
        )["data"]
        call("runs", "interrupt", interrupted["run"]["id"])
        assert call("runs", "wait", interrupted["run"]["id"])["data"]["status"] == "cancelled"
        history_followup(interrupted["thread"]["id"], interrupted["run"]["id"])
        print("Copied CLI verified HTTPS: cancelled last sealed history with normal message successor", flush=True)
        failure_prompt = os.environ.get("A13N_FAILURE_PROMPT")
        if failure_prompt:
            failed = call(
                "threads",
                "create",
                "--idempotency-key",
                str(uuid.uuid4()),
                body={"agent_id": agent, "payload": payload(failure_prompt)},
            )["data"]
            assert call("runs", "wait", failed["run"]["id"])["data"]["status"] == "failed"
            history_followup(failed["thread"]["id"], failed["run"]["id"])
            print("Copied CLI verified HTTPS: failed last sealed history with normal message successor", flush=True)
        else:
            print(
                "NOT RUN: failed-history live case requires A13N_FAILURE_PROMPT from the disposable fixture", flush=True
            )
        forked = call(
            "runs",
            "fork",
            run_id,
            "--idempotency-key",
            str(uuid.uuid4()),
            body={"agent_id": agent, "payload": payload("Fork from CLI.")},
        )["data"]
        assert forked["thread"]["id"] != thread_id, "Fork did not create its own Thread"
        assert call("runs", "wait", forked["run"]["id"])["data"]["status"] == "completed"
        waiting_id = call(
            "threads",
            "create",
            "--idempotency-key",
            str(uuid.uuid4()),
            body={
                "agent_id": client_tool_agent,
                "payload": payload("[client] Review CLI scenario."),
                "options": native_options,
            },
        )["data"]["run"]["id"]
        waiting = call("runs", "wait", waiting_id)["data"]
        assert waiting["status"] == "waiting", "Run wait must return a waiting Run without pretending success"
        pending = waiting["pending"]
        assert not pending["approvals"] and len(pending["calls"]) == 1
        resumed = call(
            "runs",
            "resume",
            waiting_id,
            "--idempotency-key",
            str(uuid.uuid4()),
            body={
                "approvals": {},
                "calls": {
                    pending["calls"][0]["tool_call_id"]: {"status": "returned", "value": {"decision": "approved"}}
                },
                "input": payload("Additional context on the reviewed CLI task."),
            },
        )["data"]
        assert waiting["options"]["configuration"] == native_options["configuration"]
        assert resumed["options"]["configuration"] == native_options["configuration"]
        assert resumed["id"] != waiting_id, "Resume must expose the successor Run identity"
        assert call("runs", "wait", waiting_id)["data"]["id"] == waiting_id, "Wait silently followed a successor"
        assert call("runs", "wait", resumed["id"])["data"]["status"] == "completed"
        print(
            "Copied CLI verified HTTPS: queued Entry wait, interrupt, fork and atomic result-plus-input resume",
            flush=True,
        )

        forbidden = execute(
            "--error-format",
            "json",
            "--organization",
            organization,
            "organizations",
            "members",
            "list",
            success=False,
        )
        assert not forbidden.stdout
        assert forbidden.returncode == 3 and json.loads(forbidden.stderr)["error"]["status"] == 403
        print(
            "Copied CLI verified HTTPS: workspace-confined credential cannot expand to account administration",
            flush=True,
        )
    print("Independent CLI binary acceptance passed; temporary consumer files removed", flush=True)


if __name__ == "__main__":
    main()
