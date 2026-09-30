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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        default=ROOT
        / "a13n-service-cli/target/debug"
        / ("a13n-service-cli.exe" if os.name == "nt" else "a13n-service-cli"),
    )
    args = parser.parse_args()
    source: Path = args.binary.resolve()
    if not source.is_file():
        raise SystemExit("Build the CLI with make cli-build or supply --binary")
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
            *arguments: str, stdin: bytes | None = None, success: bool = True, credential: bool = True
        ) -> subprocess.CompletedProcess[bytes]:
            process_environment = environment.copy()
            if not credential:
                process_environment.pop("CLI_ACCEPTANCE_TOKEN")
            result = subprocess.run(
                [str(binary), "--profile", "acceptance", "--timeout", "90", *arguments],
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
        changed = call(
            "agents", "update", agent, "--if-match", initial_etag, body={"description": "CLI acceptance"}, status=200
        )
        assert read_etag(changed) != initial_etag
        cleared = call(
            "agents", "update", agent, "--if-match", read_etag(changed), body={"description": None}, status=200
        )
        # Agent metadata explicitly treats null as keep-current, not clear-to-null.
        # Wire preservation is covered by local CLI fixtures; live behavior follows Service.
        assert cleared["data"]["description"] == "CLI acceptance", "Null changed a keep-current metadata field"
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

        history = [
            {"kind": "request", "parts": [{"part_kind": "user-prompt", "content": "Imported question."}]},
            {"kind": "response", "parts": [{"part_kind": "text", "content": "Imported answer."}]},
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
        assert first["data"]["thread"]["message_history"] == history
        run_id = first["data"]["run"]["id"]
        thread_id = first["data"]["thread"]["id"]
        assert call("threads", "get", thread_id)["data"]["message_history"] == history
        if os.name != "nt":
            checkpoint_deadline = time.monotonic() + 80
            while True:
                baseline = call("runs", "result", run_id)["data"]
                assert baseline["run"]["id"] == run_id
                if baseline["position"] is not None:
                    break
                assert time.monotonic() < checkpoint_deadline, "Run did not publish an Items baseline"
                time.sleep(0.1)
            coverage_args = ["--run", baseline["run"]["id"], "--position", baseline["position"]]
            hint = baseline.get("resume_after")
            assert hint is None or isinstance(hint, str), "Items seek hint must be absent/null/string"
            if hint is not None:
                coverage_args.extend(["--after", hint])
            events_file = directory / "events.jsonl"
            errors_file = directory / "events.stderr"
            with events_file.open("wb") as events_output, errors_file.open("wb") as events_errors:
                observer = subprocess.Popen(
                    [
                        str(binary),
                        "--profile",
                        "acceptance",
                        "threads",
                        "events",
                        "--thread",
                        thread_id,
                        *coverage_args,
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
                        if any(frame.get("cursor") is not None for frame in delivered):
                            break
                        assert observer.poll() is None, "SSE observer exited before delivering a cursor frame"
                        assert time.monotonic() < deadline, (
                            "SSE did not flush a covered boundary or tail frame while observing"
                        )
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
            items = [frame["item"] for frame in frames if frame.get("item") is not None]
            cursor_frames = [frame for frame in frames if frame.get("cursor") is not None]
            assert cursor_frames[0]["applied_position"] == baseline["position"]
            for frame in frames:
                if frame["type"] == "gap":
                    assert frame["position"] is None or isinstance(frame["position"], str)
            for item in items:
                assert item["kind"] in {"text_message", "reasoning_message", "tool_call", "observation"}
                assert item["state"] in {"in_progress", "completed", "interrupted", "failed"}
            still_running = call("runs", "get", run_id)["data"]
            assert still_running["status"] != "cancelled", "Stopping observation cancelled the durable Run"
            print(
                "Copied CLI verified direct HTTPS: Items baseline/optional hint, paired SSE coverage, flushed JSONL and local-only Ctrl-C",
                flush=True,
            )
        waited = call("runs", "wait", run_id)
        assert waited["data"]["id"] == run_id and waited["data"]["status"] == "completed"
        print(
            "Copied CLI verified HTTPS: native imported history/readback, 201/200 replay and exact Run wait", flush=True
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
        interrupted = call(
            "threads",
            "create",
            "--idempotency-key",
            str(uuid.uuid4()),
            body={"agent_id": agent, "payload": payload("[interruptible] Interrupt CLI run.")},
        )["data"]["run"]["id"]
        call("runs", "interrupt", interrupted)
        assert call("runs", "wait", interrupted)["data"]["status"] == "cancelled"
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
            body={"agent_id": client_tool_agent, "payload": payload("[client] Review CLI scenario.")},
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
