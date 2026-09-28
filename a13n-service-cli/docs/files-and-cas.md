# Transfer files and update safely

Use the [connected CLI](connection-and-profiles.md) in a workspace where you may create Assets and Memories. Binary Assets and text Memory files are different resources. A binary upload must be **published as an Asset** before its ID can be referenced in a message. Commands below change Service state; use disposable names and a fresh upload request key.

## Upload, publish and download an Asset

This POSIX-shell example uses `jq` and `cmp`, creates a small local file and checks the bytes returned by the Service:

```bash
printf 'Project report\n' > report.txt
a13n-service-cli --include-meta uploads create --file report.txt \
  --content-type text/plain --idempotency-key 'unique-upload-key' > upload.json
UPLOAD_ID=$(jq -er '.data.upload_id' upload.json)
jq -n --arg id "$UPLOAD_ID" '{name: "Project report", upload_id: $id}' > asset-create.json
a13n-service-cli --include-meta assets create --body @asset-create.json > asset.json
ASSET_ID=$(jq -er '.data.id' asset.json)
a13n-service-cli assets content get --asset "$ASSET_ID" --output downloaded.txt
cmp report.txt downloaded.txt && printf 'Downloaded bytes match\n'
```

The upload receipt returns `upload_id`; the Asset create response returns a separate `id`. The CLI streams the download through a same-directory temporary file and replaces an existing destination **only after** the full body succeeds. For stdin upload, use `--file - --file-name report.txt`; image endpoints also require an allowed explicit MIME type. `--output -` writes binary stdout but refuses an interactive terminal. Avoid reading a large Asset into an in-memory JSON string.

## Create and conditionally update a Memory file

Memory files are text under logical paths such as `notes.md`; they do not refer to files on your local disk. `MemoryCreate` needs a name, **not** a key:

```bash
a13n-service-cli --include-meta memories create \
  --body '{"name":"Project notes"}' > memory.json
MEMORY_ID=$(jq -er '.data.id' memory.json)
a13n-service-cli --include-meta memories files create --memory "$MEMORY_ID" \
  --body '{"path":"notes.md","content":"First draft"}' > file.json
ETAG=$(jq -er '.etag' file.json)
a13n-service-cli --include-meta memories files replace \
  --memory "$MEMORY_ID" --path 'notes.md' --if-match "$ETAG" \
  --body '{"content":"Updated draft"}' > updated.json
jq '.data.content' updated.json
```

Pass the returned ETag **including its quotes**. A stale `If-Match` returns `412`: read the current file and reconcile; do not retry by silently replacing the ETag. The same conditional-read-before-write pattern applies to an Agent update:

```bash
AGENT_ID='your-disposable-agent-id'
a13n-service-cli --include-meta agents get "$AGENT_ID" > agent.json
ETAG=$(jq -er '.etag' agent.json)
a13n-service-cli agents update "$AGENT_ID" --if-match "$ETAG" \
  --body '{"description":"Reviewed"}'
```

That final command modifies an Agent; point it only at an Agent you intend to edit. Omitted and `null` fields are distinct on the wire, but **null does not universally mean clear**—check the target field's Service semantics. For a catalog with cursors, `agents list --limit 10` reads one page and `agents list --limit 10 --all --include-meta` returns `{"pages":[...]}` with one response envelope per page. Memory revision selectors are integers; restore can return `file: null` after undoing creation. To mount a Memory into an Agent Run, use the SDK's [files and Memory guide](../../docs/files-and-memory.md).
