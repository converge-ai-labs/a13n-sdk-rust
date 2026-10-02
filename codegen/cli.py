"""Generate CLI commands and typed SDK dispatch from the pinned resource graph."""

import json
import re
from pathlib import Path

from resources import enum_name, enum_values, identifier, snake


def q(value: str) -> str:
    return json.dumps(value)


def route(op: dict, owner: str) -> list[str]:
    parts = [part for part in op["relative"].split("/") if not part.startswith("{")]
    if parts[:1] == ["workspaces"] and "{workspace_id}" in op["relative"]:
        # Retain roots where they disambiguate organization/global siblings.
        if len(parts) > 1 and parts[1] not in {"connections", "invitations"}:
            parts = parts[1:]
    if owner != op["relative"]:
        parts.pop()  # POST-only action leaf flattened onto its parent SDK resource.
    return [*parts, op["method"]]


def path_params(op: dict) -> list[str]:
    return re.findall(r"\{(\w+)\}", op["path"])


def terminal(op: dict) -> str | None:
    params = path_params(op)
    if not params:
        return None
    last = params[-1]
    if last == "path":
        return None  # file paths are explicit flags, including Unicode and slashes.
    after = op["path"].split("{" + last + "}", 1)[-1]
    # IDs for a nested resource are flags; IDs for an action on that resource
    # remain positional (e.g. runs resume ID rather than runs resume --run ID).
    if after and op["method"] in {"list", "get", "create", "update", "replace", "delete"}:
        return None
    if last in {"workspace_id", "organization_id"} and after:
        return None
    return last


def field_type(field: str) -> str:
    match = re.search(r"pub (?:r#)?\w+:(.*),", field)
    assert match is not None, field
    return match.group(1).strip()


def field_name(field: str) -> str:
    match = re.search(r"pub ((?:r#)?\w+):", field)
    assert match is not None, field
    return match.group(1).removeprefix("r#")


def own_parameters(test: dict) -> list[tuple[str, str, bool]]:
    return [
        (field_name(field), field_type(field), not field_type(field).startswith("Option<")) for field in test["fields"]
    ]


def example_template(test: dict) -> str:
    op = test["op"]
    tokens = ["a13n-service-cli"]
    for scope in ("workspace_id", "organization_id"):
        if scope in path_params(op) and terminal(op) != scope:
            tokens += [f"--{scope.removesuffix('_id')}", scope.upper()]
    tokens += route(op, test["owner"])
    target = terminal(op)
    if target:
        tokens.append(target.upper())
    for parameter in path_params(op):
        if parameter not in {terminal(op), "workspace_id", "organization_id"}:
            tokens += [f"--{parameter.removesuffix('_id').replace('_', '-')}", parameter.upper()]
    for field, _, required in own_parameters(test):
        if required:
            tokens += [f"--{field.replace('_', '-')}", field.upper()]
    media = op.get("requestBody", {}).get("content", {})
    if "application/json" in media:
        tokens += ["--body", "@request.json"]
    elif media:
        tokens += ["--file", "FILE"]
        if "multipart/form-data" in media:
            tokens += ["--content-type", "MIME"]
    if op["result"] is None:
        tokens += ["--output", "FILE"]
    return " ".join(tokens)


def command(trie: dict, name: str, op_tests: list[dict], definitions: list[str]) -> str:
    node = trie
    slot = len(definitions)
    function_name = f"group_{slot}"
    definitions.append("")
    text = f"let mut node = Command::new({q(name)}).subcommand_required(true).arg_required_else_help(true);"
    for child, value in node.items():
        if child == "#":
            continue
        if "#" in value:
            index = value["#"]
            op = op_tests[index]["op"]
            test = op_tests[index]
            details = op.get("description", op.get("summary", "Service operation"))
            details += f" ({op['verb'].upper()} {op['path']}). Use --schema for the offline request schema."
            if any(p["name"] == "If-Match" for p in op["parameters"]):
                details += " Pass the target ETag explicitly; stale writes fail without automatic replay."
            if any(p["name"] == "Idempotency-Key" for p in op["parameters"]):
                details += " Supply a caller-owned idempotency key; an uncertain outcome needs readback."
            details += f"\n\nExample template (replace uppercase placeholders and provide your own request body):\n  {example_template(test)}"
            leaf = (
                f"Command::new({q(child)}).about({q(op.get('summary', 'Service operation'))}).long_about({q(details)})"
            )
            term = terminal(op)
            if term:
                leaf += f'.arg(Arg::new("id").required_unless_present_any(["schema", "example"]).help({q(term)}))'
            for param in path_params(op):
                if param != term:
                    if param in {"workspace_id", "organization_id"}:
                        continue
                    leaf += f'.arg(Arg::new({q(param)}).long({q(param.removesuffix("_id").replace("_", "-"))}).required_unless_present_any(["schema", "example"]))'
            for field, typ, required in own_parameters(test):
                if field == "content_type" and op.get("requestBody", {}).get("content", {}).get("multipart/form-data"):
                    continue
                flag = (
                    "filter-workspace"
                    if field == "workspace_id"
                    and any(p["name"] == "workspace_id" and p["in"] == "query" for p in op["parameters"])
                    else field.replace("_", "-")
                )
                leaf += f".arg(Arg::new({q(field)}).long({q(flag)})"
                parameter = next((p for p in op["parameters"] if snake(p["name"]) == field), None)
                if parameter:
                    schema = parameter["schema"]
                    for branch in schema.get("anyOf", []):
                        if branch.get("type") in {"integer", "number"}:
                            schema = branch
                            break
                    allowed = enum_values(schema)
                    if allowed:
                        leaf += f".value_parser([{', '.join(q(v) for v in allowed)}])"
                    minimum = schema.get("minimum")
                    if schema.get("type") == "integer" and isinstance(minimum, int):
                        leaf += f'.value_parser(clap::builder::ValueParser::new(|s: &str| -> Result<String,String> {{ if s.parse::<i64>().is_ok_and(|n| n >= {minimum}) {{ Ok(s.to_owned()) }} else {{ Err("expected integer >= {minimum}".into()) }} }}))'
                if required:
                    leaf += '.required_unless_present_any(["schema", "example"])'
                if "Vec<" in typ:
                    leaf += ".action(ArgAction::Append)"
                leaf += ")"
            media = op.get("requestBody", {}).get("content", {})
            if "application/json" in media:
                leaf += '.arg(Arg::new("body").long("body").required_unless_present_any(["schema", "example"]).help("JSON, @file or @- for stdin"))'
            elif media:
                leaf += '.arg(Arg::new("file").long("file").required_unless_present_any(["schema", "example"]).help("File path or - for stdin"))'
                if "multipart/form-data" in media:
                    leaf += '.arg(Arg::new("file_name").long("file-name")).arg(Arg::new("content_type").long("content-type").required_unless_present_any(["schema", "example"]))'
            if test["op"]["result"] is None:
                leaf += '.arg(Arg::new("output").long("output").help("File path or - for stdout (required for binary content)"))'
            if test["method"] == "list" and any(p["name"] == "cursor" for p in op["parameters"]):
                leaf += '.arg(Arg::new("all").long("all").action(ArgAction::SetTrue).help("Fetch all cursor pages; output pages array"))'
            leaf += '.arg(Arg::new("schema").long("schema").action(ArgAction::SetTrue)).arg(Arg::new("example").long("example").action(ArgAction::SetTrue))'
            definitions.append(f"fn operation_{index}() -> Command {{ {leaf} }}")
            text += f" node = node.subcommand(operation_{index}());"
        else:
            child_function = command(value, child, op_tests, definitions)
            text += f" node = node.subcommand({child_function}());"
    definitions[slot] = f"fn {function_name}() -> Command {{ {text} node }}"
    return function_name


def chain(op: dict, owner: str) -> str:
    parts = owner.split("/") if owner else []
    expr = "client.resources()"
    for part in parts:
        if part.startswith("{"):
            param = part[1:-1]
            source = "id" if terminal(op) == param else param
            if param in {"workspace_id", "organization_id"} and source == param and terminal(op) != param:
                value = f"config.required({q(param)})?"
            else:
                value = f"required(matches, {q(source)})?"
            # At endpoints for integer path segments receive integer IDs, not strings.
            spec = next((p for p in op["parameters"] if p["name"] == param), None)
            if spec and enum_values(spec["schema"]):
                value = f"input::option::<{enum_name(op['path'], spec, 'ProviderKindResource')}>({value})?"
            elif op["signature"].get(snake(param)) in ("i32", "i64"):
                value = f'{value}.parse().map_err(|_| CliError::input("invalid numeric path ID"))?'
            expr += f".at({value})" if part.startswith("{") else ""
        else:
            expr += f".{identifier(snake(part))}()"
    return expr


def dispatch_case(index: int, test: dict) -> str:
    op = test["op"]
    owner = test["owner"]
    fields = own_parameters(test)
    setup = []
    args = []
    media = op.get("requestBody", {}).get("content", {})
    if "application/json" in media:
        excluded = {snake(p["name"]) for p in op["parameters"]} | {"configuration"}
        typ = next(t for name, t in op["signature"].items() if name not in excluded)
        setup += [
            'let value = input::body(required(matches, "body")?).await?;',
            f"input::validate({index}, &value)?;",
            f'let body: {typ} = serde_json::from_value(value).map_err(|_| CliError::input("request body does not match --schema"))?;',
        ]
        args.append("&body")
    elif "multipart/form-data" in media:
        setup.append("let file = input::upload(matches).await?;")
        args.append("file")
    elif media:
        setup.append("let body = input::raw_upload(matches).await?;")
        args.append("body")
    if fields:
        expressions = []
        for field, typ, required_field in fields:
            if field == "content_type" and media and "multipart/form-data" not in media:
                value = 'required(matches, "content_type")?.to_owned()'
            elif required_field:
                value = (
                    f"input::option(required(matches, {q(field)})?)?"
                    if typ != "String"
                    else f"required(matches, {q(field)})?.to_owned()"
                )
            elif "Vec<" in typ:
                value = f"input::many(matches, {q(field)})?"
            else:
                value = f"input::optional(matches, {q(field)})?"
            expressions.append(f"{identifier(field)}: {value}")
        setup.append(f"let options = {test['options']} {{ {', '.join(expressions)} }};")
        args.append("options")
    call = f"{chain(op, owner)}.{test['method']}({', '.join(args)})"
    if op["result"] is None:
        result = f"let response = {call}.await?; output::binary(response, matches).await"
    elif test["method"] == "list" and any(p["name"] == "cursor" for p in op["parameters"]):
        result = f'if matches.get_flag("all") {{ output::pages({chain(op, owner)}.pages(options), config).await }} else {{let response = {call}.await?; output::response(response, config).await}}'
        # options is moved into .list only in the else branch.
    else:
        result = f"let response = {call}.await?; output::response(response, config).await"
    return f"{index} => {{ {' '.join(setup)} {result} }}"


def generate_cli(document: dict, nodes: dict, tests: list[dict], target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    trie = {}
    for index, test in enumerate(tests):
        op = test["op"]
        op["method"] = test["method"].removeprefix("r#")
        segments = route(op, test["owner"])
        node = trie
        for segment in segments:
            node = node.setdefault(segment, {})
        assert "#" not in node, (segments, index, node["#"])
        node["#"] = index
    names = []
    for test in tests:
        names.append(" ".join(route(test["op"], test["owner"])))
    assert len(names) == len(set(names)), "CLI command collision"
    # This one compact schema document remains CLI-owned; SDK crates never include it.
    schemas = {
        "components": document["components"]["schemas"],
        "requests": {
            str(index): test["op"].get("requestBody", {}).get("content", {}).get("application/json", {}).get("schema")
            for index, test in enumerate(tests)
        },
        "media_requests": {
            str(index): test["op"].get("requestBody")
            for index, test in enumerate(tests)
            if "application/json" not in test["op"].get("requestBody", {}).get("content", {})
        },
    }
    (target.parent / "schemas.json").write_text(json.dumps(schemas, separators=(",", ":")) + "\n")
    entries = ",\n".join(
        f"({q(name)}, {q(test['op']['verb'].upper())}, {q(test['op']['path'])})"
        for name, test in zip(names, tests, strict=True)
    )
    command_functions: list[str] = []
    root = command(trie, "api", tests, command_functions)
    lines = [
        "// Generated by codegen/cli.py from pinned OpenAPI + ordinary SDK resource graph. Do not edit.",
        "use a13n::{Client, generated::models, resources::*};",
        "use clap::{Arg, ArgAction, ArgMatches, Command};",
        "use crate::{CliError, config::Effective, input, output, required};",
        f"pub const OPERATIONS: &[(&str,&str,&str)] = &[{entries}];",
        "pub const BINARY_OPERATIONS: &[usize] = &["
        + ",".join(str(index) for index, test in enumerate(tests) if test["op"]["result"] is None)
        + "];",
        "pub const EXAMPLES: &[&str] = &[" + ",".join(q(example_template(test)) for test in tests) + "];",
        *command_functions,
        f"pub fn commands() -> Command {{ {root}() }}",
        "pub fn selected(matches: &ArgMatches) -> Option<(usize, &ArgMatches)> {",
        "let mut names = Vec::new(); let mut current = matches; while let Some((name, child)) = current.subcommand() { names.push(name); current = child; }",
        'let name = names.join(" " ); OPERATIONS.iter().position(|item| item.0 == name).map(|index| (index, current)) }',
        "pub async fn dispatch(index: usize, matches: &ArgMatches, client: &Client, config: &Effective) -> Result<(), CliError> { match index {",
        ",\n".join(dispatch_case(index, test) for index, test in enumerate(tests)),
        '_ => Err(CliError::input("unknown operation")) }}',
    ]
    target.write_text("\n".join(lines) + "\n")
