"""Emit callable dispatch tests alongside the resource graph for automatic sync."""

import json
from pathlib import Path


def sample(schema: dict, schemas: dict, depth: int = 0):
    if depth > 20:
        return None
    if "$ref" in schema:
        return sample(schemas[schema["$ref"].split("/")[-1]], schemas, depth + 1)
    if "const" in schema:
        return schema["const"]
    if "enum" in schema:
        return schema["enum"][0]
    for union in ["oneOf", "anyOf"]:
        if union in schema:
            branches = schema[union]
            return sample(next((s for s in branches if s.get("type") != "null"), branches[0]), schemas, depth + 1)
    kind = schema.get("type")
    if kind == "object" or "properties" in schema:
        return {key: sample(schema["properties"][key], schemas, depth + 1) for key in schema.get("required", [])}
    if kind == "array":
        return []
    if kind in {"integer", "number"}:
        return 1
    if kind == "boolean":
        return True
    if kind == "null":
        return None
    if kind == "string":
        return {
            "date-time": "2026-09-26T00:00:00Z",
            "date": "2026-09-26",
            "uuid": "00000000-0000-4000-8000-000000000001",
        }.get(schema.get("format", ""), "test")
    return {}


def rust_json(value) -> str:
    return (
        "serde_json::from_str(" + json.dumps(json.dumps(value, ensure_ascii=False), ensure_ascii=False) + ").unwrap()"
    )


def generate_tests(document: dict, nodes: dict, tests: list, output: Path) -> None:
    # Import lazily: the graph module calls this after its own definitions exist.
    from resources import enum_name, enum_values, identifier, pascal, snake

    schemas = document["components"]["schemas"]
    source = [
        "// Generated dispatch coverage. Included by tests/coverage.rs.",
        "use a13n::{resources::*, UploadFile};",
        "use common::{Reply, server, client};",
    ]
    for test in tests:
        op = test["op"]
        chain = "client.resources()"
        current = ""
        expected = op["path"]
        for segment in test["owner"].split("/") if test["owner"] else []:
            if segment.startswith("{"):
                param = next(p for p in op["parameters"] if p["name"] == segment[1:-1])
                schema = param["schema"]
                if enum_values(schema):
                    value = (
                        enum_name(op["path"], param, nodes[test["owner"]]["name"])
                        + "::"
                        + pascal(enum_values(schema)[0])
                    )
                    wire = enum_values(schema)[0]
                elif schema.get("type") == "integer":
                    value, wire = "1", "1"
                else:
                    value, wire = '"id/part"', "id%2Fpart"
                chain += f".at({value})"
                expected = expected.replace(segment, wire)
            else:
                chain += f".{identifier(snake(segment))}()"
            current = f"{current}/{segment}".lstrip("/")
        args = []
        for arg in test["args"]:
            name, typ = arg.split(":", 1)
            if name == "options":
                fields = []
                for declaration in test["fields"]:
                    field, field_type = declaration.removeprefix("pub ").rstrip(",").split(":", 1)
                    if field == "content_type":
                        value = json.dumps(next(iter(op["requestBody"]["content"]))) + ".into()"
                    elif field_type == "String":
                        value = '"test".into()'
                    else:
                        parameter = next((p for p in op["parameters"] if identifier(snake(p["name"])) == field), None)
                        if parameter and enum_values(parameter["schema"]):
                            value = rust_json(enum_values(parameter["schema"])[0])
                        else:
                            value = "Default::default()"
                    fields.append(f"{field}:{value}")
                args.append(typ + "{" + ",".join(fields) + "}")
            elif name == "file":
                args.append(
                    'UploadFile{name:"sample.bin".into(),content_type:"application/octet-stream".into(),body:vec![1,2,3].into()}'
                )
            elif typ == "reqwest::Body":
                args.append("vec![1,2,3].into()")
            else:
                args.append(
                    "&" + rust_json(sample(op["requestBody"]["content"]["application/json"]["schema"], schemas))
                )
        invoke = f"{chain}.{test['method']}({','.join(args)}).await.unwrap()"
        cases = []
        for code, response in op["responses"].items():
            if not code.startswith("2"):
                continue
            media = response.get("content", {})
            if "application/json" in media:
                reply = f"Reply::json({code},{rust_json(sample(media['application/json']['schema'], schemas))})"
            elif media:
                reply = f"Reply::bytes({code},{json.dumps(next(iter(media)))},vec![1,2,3])"
            else:
                reply = f'Reply::bytes({code},"application/octet-stream",Vec::new())'
            result = "response.receipt" if op["result"] == "models::Submitted" else "response"
            cases.append(
                "{"
                + f'let mut server=server(|_|{reply}).await;let client=client(&server);let response={invoke};assert_eq!({result}.status.as_u16(),{code});assert_eq!({result}.headers["x-request-id"],"req_test");let request=server.requests.recv().await.unwrap();assert_eq!(request.method,{json.dumps(op["verb"].upper())});assert_eq!(request.target.split(\'?\').next().unwrap(),{json.dumps("/prefix" + expected)});'
                + "}"
            )
        source.append(f"#[tokio::test] async fn {snake(op['operationId'])}() {{" + "".join(cases) + "}")
    (output / "resource_tests.rs").write_text("\n".join(source) + "\n")
