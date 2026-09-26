"""Generate a complete ordinary resource graph using the generated wire types."""

import json
import re
from pathlib import Path

from resource_tests import generate_tests

VERBS = {"get": "get", "post": "create", "put": "replace", "patch": "update", "delete": "delete"}
CORE = {
    "workspaces/{workspace_id}": "WorkspaceResource",
    "workspaces/{workspace_id}/threads": "ThreadsResource",
    "workspaces/{workspace_id}/threads/{thread_id}": "ThreadResource",
    "workspaces/{workspace_id}/threads/{thread_id}/inbox": "InboxResource",
    "workspaces/{workspace_id}/threads/{thread_id}/inbox/{entry_id}": "EntryResource",
    "workspaces/{workspace_id}/runs/{run_id}": "RunResource",
}
OPTIONAL_MATCH = "/api/v1/workspaces/{workspace_id}/memories/{memory_id}/revisions/{seq}/restore"
DOMAIN_ENUMS = {
    ("/api/v1/provider-types/{kind}", "kind"): "ProviderKind",
    ("/api/v1/organizations/{organization_id}/members", "kind"): "MemberKind",
    ("/api/v1/workspaces/{workspace_id}/skills", "source"): "SkillSource",
}


def rustdoc(text: str) -> str:
    return "\n".join("/// " + line if line else "///" for line in text.splitlines()) + "\n"


def enum_values(schema: dict) -> list[str]:
    if "enum" in schema:
        return schema["enum"]
    for branch in schema.get("anyOf", []):
        if "enum" in branch:
            return branch["enum"]
    return []


def enum_name(path: str, parameter: dict, owner: str) -> str:
    return DOMAIN_ENUMS.get((path, parameter["name"]), owner.removesuffix("Resource") + pascal(parameter["name"]))


def parameter_doc(parameter: dict, required: bool) -> str:
    text = f"{parameter['in'].capitalize()} parameter `{parameter['name']}`."
    if parameter.get("description"):
        text += " " + " ".join(parameter["description"].split())
    if parameter["name"] == "If-Match":
        text += " Use the ETag from the resource being changed; the SDK never infers it."
        if not required:
            text += " Omit only when restoring an absent target."
    elif parameter["name"] == "Idempotency-Key":
        text += " Caller-owned request key; uncertain mutations are not automatically replayed."
    if required:
        text += (
            " Required; an empty string is rejected locally."
            if parameter["schema"].get("type") == "string" and not enum_values(parameter["schema"])
            else " Required."
        )
    else:
        text += " `None` omits this parameter."
    return text


def operation_doc(op: dict, public: str) -> str:
    lines = [
        op.get("summary", "Perform this Service operation.").rstrip(".") + ".",
        f"`{op['verb'].upper()} {op['path']}`.",
    ]
    if public == "Submitted<'a>":
        lines.append(
            "Returns an acceptance receipt with canonical Thread/Entry references and an optional Run. A queued Entry has no Run; acceptance is not completion."
        )
    elif public == "BinaryResponse":
        lines.append("Returns an unbuffered body. Read chunks or drop/close it; reads share parent shutdown.")
        if op["path"].endswith("/threads/{thread_id}/stream"):
            lines.append(
                "For typed frames and applied-cursor recovery, use [`ThreadResource::events`] instead of this raw stream."
            )
    else:
        lines.append("Preserves the actual HTTP status and headers, including ETag and request ID.")
    if "application/json" in op.get("requestBody", {}).get("content", {}):
        lines.append(
            "Accepts the full generated request model. Optional nullable fields distinguish omission (`None`), null (`Some(None)`) and value (`Some(Some(value))`)."
        )
    if op["path"].endswith("/runs/{run_id}/resume"):
        lines.append(
            "Returns the successor Run. Bind its returned ID before waiting; waiting on the original Run does not follow successors."
        )
    if any(p["name"] == "If-Match" for p in op["parameters"]):
        lines.append("Pass the target resource's ETag explicitly; Service validates stale preconditions.")
    lines.append(
        "Drop or time out the whole future to stop local work. Cancellation does not prove Service rollback; mutations are not automatically retried."
    )
    return rustdoc("\n\n".join(lines))


def pascal(value: str) -> str:
    return "".join(word[0].upper() + word[1:] for word in re.split(r"[^a-zA-Z0-9]+", value) if word)


def snake(value: str) -> str:
    return re.sub(r"[^a-zA-Z0-9]+", "_", value).lower()


def identifier(value: str) -> str:
    # Raw identifiers keep wire names recognizable without colliding with Rust keywords.
    reserved = set(
        "as break const continue crate else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while async await dyn abstract become box do final macro override priv typeof unsized virtual yield try gen".split()
    )
    if value in {"self", "Self", "super", "crate"}:
        return value + "_"
    return "r#" + value if value in reserved else value


def resource_name(path: str) -> str:
    if path in CORE:
        return CORE[path]
    parts = []
    for segment in path.split("/"):
        if segment.startswith("{"):
            word = parts.pop()
            parts.append(word[:-3] + "y" if word.endswith("ies") else word[:-1] if word.endswith("s") else word)
        else:
            parts.append(segment)
    if (
        len(parts) > 1
        and parts[0] == "workspace"
        and parts[1] not in {"connection", "connections", "invitation", "invitations"}
    ):
        parts = parts[1:]
    return "".join(pascal(part) for part in parts) + "Resource"


def generate_resources(document: dict, output: Path) -> None:
    bindings = "\n".join(p.read_text() for p in (output / "apis").glob("*.rs"))
    signatures = {}
    for match in re.finditer(r"pub async fn (\w+)\((.*?)\)\s*->\s*Result<(.*?)>\s*\{", bindings, re.S):
        arguments = {
            name.removeprefix("r#"): typ.strip()
            for name, typ in re.findall(r"\b((?:r#)?\w+): (.*?)(?=,\s*(?:r#)?\w+:|,\s*$|$)", match[2], re.S)
        }
        returned = re.search(r"Response<(.*?)>,\s*Error", match[3], re.S)
        signatures[match[1]] = (arguments, returned[1].strip() if returned else None)
    nodes: dict[str, dict] = {"": {"name": "ServiceResources", "children": {}, "ops": []}}
    operations = []
    for path, item in document["paths"].items():
        relative = path.removeprefix("/api/v1/") if path.startswith("/api/v1/") else path.lstrip("/")
        parent = ""
        for segment in relative.split("/"):
            key = f"{parent}/{segment}".lstrip("/")
            nodes[parent]["children"][segment] = key
            nodes.setdefault(key, {"name": resource_name(key), "children": {}, "ops": []})
            parent = key
        for verb, operation in item.items():
            if verb not in VERBS:
                continue
            signature, result = signatures[snake(operation["operationId"])]
            op = dict(operation, path=path, relative=relative, verb=verb, signature=signature, result=result)
            op["parameters"] = item.get("parameters", []) + operation.get("parameters", [])
            operations.append(op)
            nodes[parent]["ops"].append(op)
    for key, node in nodes.items():
        node["flatten"] = bool(
            key
            and not key.endswith(("}", "s"))
            and not node["children"]
            and len(node["ops"]) == 1
            and node["ops"][0]["verb"] == "post"
        )
    names = [node["name"] for node in nodes.values()]
    assert len(names) == len(set(names)), "resource name collision"
    source = [
        "// Generated by codegen/resources.py. Do not edit.",
        "use crate::{client::*, resources::*, interaction::Submitted, generated::models};",
    ]
    inventory = []
    tests = []
    enums: dict[str, list[str]] = {}
    for op in operations:
        for parameter in op["parameters"]:
            values = enum_values(parameter["schema"])
            if not values:
                continue
            name = enum_name(op["path"], parameter, nodes[op["relative"]]["name"])
            assert name not in enums or enums[name] == values, "parameter enum collision"
            if name in enums:
                continue
            enums[name] = values
            source.append(
                rustdoc(f"Allowed values for the `{parameter['name']}` selector. Serialized using Service wire values.")
            )
            source.append(
                f"#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)] pub enum {name} {{"
                + ",".join(f"#[serde(rename = {json.dumps(value)})] {pascal(value)}" for value in values)
                + "}"
            )
            source.append(
                f"impl {name} {{ pub fn as_str(self)-> &'static str {{match self {{"
                + ",".join(f"Self::{pascal(value)}=>{json.dumps(value)}" for value in values)
                + "}}}"
            )
            source.append(
                f"impl std::fmt::Display for {name} {{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {{f.write_str(self.as_str())}}}}"
            )
    for key, node in nodes.items():
        if node["flatten"]:
            continue
        name = node["name"]
        source.append(
            rustdoc(
                "A local resource reference borrowing its client's transport and shutdown lifetime. Binding performs no request and grants no additional authority."
            )
        )
        source.append(f"#[derive(Clone)] pub struct {name}<'a>(pub(crate) Binding<'a>);")
        methods = []
        for segment, child_key in node["children"].items():
            child = nodes[child_key]
            if child["flatten"]:
                continue
            if segment.startswith("{"):
                parameter = segment[1:-1]
                op = next(
                    op for op in operations if op["relative"] == child_key or op["relative"].startswith(child_key + "/")
                )
                spec = next(p for p in op["parameters"] if p["name"] == parameter)
                typ = op["signature"][snake(parameter)]
                if typ == "&str":
                    typ, value = "impl Into<String>", "id.into()"
                else:
                    value = "id.to_string()"
                if enum_values(spec["schema"]):
                    typ = enum_name(op["path"], spec, child["name"])
                    value = "id.as_str().into()"
                methods.append(
                    rustdoc(
                        f"Bind `{parameter}` locally without an HTTP request. The returned reference borrows the client, not this collection."
                    )
                    + f"pub fn at(&self,id:{typ})->{child['name']}<'a> {{{child['name']}(self.0.select({value}))}}"
                )
            else:
                methods.append(
                    rustdoc(f"Access `{segment}` locally, sharing the client and explicit scope.")
                    + f"pub fn {identifier(snake(segment))}(&self)->{child['name']}<'a> {{{child['name']}(self.0.clone())}}"
                )
        own_ops = [(op, VERBS[op["verb"]]) for op in node["ops"]]
        own_ops += [
            (nodes[child]["ops"][0], snake(segment))
            for segment, child in node["children"].items()
            if nodes[child]["flatten"]
        ]
        method_names = set()
        for op, method in own_ops:
            responses = [(code, response) for code, response in op["responses"].items() if code.startswith("2")]
            schema = next(
                (r.get("content", {}).get("application/json", {}).get("schema", {}) for _, r in responses), {}
            )
            if "$ref" in schema:
                schema = document["components"]["schemas"][schema["$ref"].split("/")[-1]]
            if (
                op["verb"] == "get"
                and "items" in schema.get("properties", {})
                and not op["path"].endswith("/runs/{run_id}/items")
            ):
                method = "list"
            assert method not in method_names, (name, method)
            method_names.add(method)
            status = [int(code) for code, _ in responses]
            if op["path"] == "/api/v1/connections/callback":
                status.append(303)
            args = []
            fields = []
            field_docs = []
            setup = []
            call = [
                f"let mut request=self.0.client.request(reqwest::Method::{op['verb'].upper()},{json.dumps(op['path'])},&self.0.ids)?;"
            ]
            option_name = name.removesuffix("Resource") + pascal(method) + "Options"
            parameters = [p for p in op["parameters"] if p["in"] != "path"]
            for p in parameters:
                field = snake(p["name"])
                typ = op["signature"][field].replace("&str", "String")
                field = identifier(field)
                required = p.get("required", False) or (p["name"] == "If-Match" and op["path"] != OPTIONAL_MATCH)
                if required and typ.startswith("Option<"):
                    typ = typ[7:-1]
                if enum_values(p["schema"]):
                    selected = enum_name(op["path"], p, name)
                    typ = selected if required else f"Option<{selected}>"
                fields.append(f"pub {field}:{typ},")
                field_docs.append(rustdoc(parameter_doc(p, required)))
                if required and typ == "String":
                    setup.append(f"if options.{field}.is_empty() {{return Err(Error::InvalidInput)}}")
                expr = f"&options.{field}" if required else "value"
                line = (
                    f"request=request.header({json.dumps(p['name'])},{expr});"
                    if p["in"] == "header"
                    else f"request=query(request,{json.dumps(p['name'])},{expr})?;"
                )
                call.append(line if required else f"if let Some(value)=&options.{field} {{{line}}}")
            media = op.get("requestBody", {}).get("content", {})
            if "application/json" in media:
                excluded = {snake(p["name"]) for p in op["parameters"]} | {"configuration"}
                body_type = next(t for param, t in op["signature"].items() if param not in excluded)
                args.append(f"body:&{body_type}")
                call.append("request=request.json(body);")
            elif "multipart/form-data" in media:
                args.append("file:UploadFile")
                call.append("request=request.multipart(file.form()?);")
            elif media:
                args.append("body:reqwest::Body")
                fields.append("pub content_type:String,")
                field_docs.append(
                    rustdoc(
                        "Explicit image MIME type. Allowed values: " + ", ".join(f"`{kind}`" for kind in media) + "."
                    )
                )
                setup.append(
                    f"if ![{','.join(json.dumps(m) for m in media)}].contains(&options.content_type.as_str()) {{return Err(Error::InvalidInput)}}"
                )
                call.append('request=request.header("Content-Type",options.content_type).body(body);')
            if fields:
                source.append(
                    rustdoc(
                        f"Query and header options for [`{name}::{method}`]. Required values must be supplied before calling the method."
                    )
                    + f"#[derive(Clone, Debug, Default)] pub struct {option_name} {{\n"
                    + "\n".join(doc + field for doc, field in zip(field_docs, fields, strict=True))
                    + "\n}"
                )
                args.append(f"options:{option_name}")
            if len(call) == 1:
                call[0] = call[0].replace("let mut request=", "let request=")
            result_type = op["result"]
            submitted = result_type == "models::Submitted"
            public = (
                "BinaryResponse"
                if result_type is None
                else "Submitted<'a>"
                if submitted
                else f"Response<{result_type}>"
            )
            call.append(
                f"let response=self.0.client.{'send' if result_type is None else 'json'}(request,&{status}).await?;"
            )
            call.append("Submitted::bind(self.0.client,response)" if submitted else "Ok(response)")
            methods.append(
                operation_doc(op, public)
                + f"pub async fn {identifier(method)}(&self{',' if args else ''}{','.join(args)})->Result<{public},Error> {{"
                + "\n".join(setup + call)
                + "}"
            )
            inventory.append((op["verb"].upper(), op["path"], name, method))
            tests.append(
                dict(op=op, owner=key, method=identifier(method), args=args, fields=fields, options=option_name)
            )
            if (
                method == "list"
                and any(p["name"] == "cursor" for p in parameters)
                and "next_cursor" in schema.get("properties", {})
            ):
                pager = name.removesuffix("Resource") + "Pages"
                methods.append(
                    rustdoc(
                        "Iterate cursor pages lazily with owned options. Each page retains status and headers; no prefetch occurs. Drop the pager to stop reads."
                    )
                    + f"pub fn pages(&self,options:{option_name})->Pages<{pager}<'a>> {{let cursor=options.cursor.clone(); Pages::new({pager}{{resource:self.clone(),options}},cursor)}}"
                )
                source.append(
                    rustdoc(f"Page source owned by [`{name}::pages`].")
                    + f"pub struct {pager}<'a> {{resource:{name}<'a>,options:{option_name}}}"
                )
                source.append(
                    f"impl PageSource for {pager}<'_> {{type Page={result_type}; async fn fetch(&self,cursor:Option<String>)->Result<Response<Self::Page>,Error>{{let mut options=self.options.clone();options.cursor=cursor;self.resource.list(options).await}} fn cursor(page:&Self::Page)->Option<String>{{page.next_cursor.clone()}}}}"
                )
        source.append(f"impl<'a> {name}<'a> {{" + "\n".join(methods) + "}")
    source.append(
        "pub const RESOURCE_OPERATIONS: &[(&str,&str,&str,&str)] = &["
        + ",".join("(" + ",".join(json.dumps(v) for v in row) + ")" for row in inventory)
        + "];"
    )
    (output / "resources.rs").write_text("\n".join(source) + "\n")
    generate_tests(document, nodes, tests, output)
