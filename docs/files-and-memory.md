# Attach a file and mount Memory

Assets are binary uploads; Memory files are editable text held in a Memory. An upload ID alone is **not** an Asset ID: publish the upload as an Asset before adding it to a message. To make Memory available to a Run, mount the Memory ID when starting the Thread.

You need the [source-installed SDK](../README.md#get-started-from-source), Service URL, workspace API key, existing Agent ID, two distinct keys (`A13N_UPLOAD_KEY` and `A13N_REQUEST_KEY`), and a local UTF-8 file to attach. From the sibling application's directory, create one with `printf 'Project notes\n' > report.txt`. The example creates an upload, Asset and Memory on the Service; use disposable state and a Model capable of working with your chosen attachment.

```rust
use a13n::{Client, Secret, StartOptions, Input, InputPart, UploadFile, generated::models, resources::MemoryFileReplaceOptions};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let original = std::fs::read("report.txt")?;
    let upload = client.api().uploads().create(
        UploadFile {
            name: "report.txt".into(),
            content_type: "text/plain".into(),
            body: original.clone().into(),
        },
        a13n::resources::UploadsCreateOptions {
            idempotency_key: env::var("A13N_UPLOAD_KEY")?,
            ..Default::default()
        },
    ).await?;
    let asset = client.api().assets().create(
        &models::AssetCreate::new("Project report".into(), upload.data.upload_id),
        Default::default(),
    ).await?;

    let memory = client.api().memories().create(
        &models::MemoryCreate::new("Project notes".into()), Default::default(),
    ).await?;
    client.api().memories().at(&memory.data.id).files().create(
        &models::MemoryFileCreate::new("Read the project report.".into(), "notes.md".into()),
        Default::default(),
    ).await?;
    let file = client.api().memories().at(&memory.data.id).files().at("notes.md");
    let current = file.get(Default::default()).await?;
    let etag = current.etag().ok_or("Memory file has no ETag")?.to_owned();
    let updated = file.replace(
        &models::MemoryFileReplace::new(format!("{}\nRecord open questions.", current.data.content)),
        MemoryFileReplaceOptions { if_match: etag, ..Default::default() },
    ).await?;
    println!("Memory file updated: HTTP {}", updated.status);

    let input = Input::parts([
        InputPart::text("Summarize the attached report using project notes."),
        InputPart::asset(&asset.data.id),
    ]);
    let options = StartOptions {
        idempotency_key: Some(env::var("A13N_REQUEST_KEY")?),
        memories: Some(vec![models::MemoryMount::new(
            models::MemoryAccess::Read, memory.data.id, "notes".into(),
        )]),
        ..Default::default()
    };
    let mut interaction = client.agent(env::var("A13N_AGENT_ID")?)
        .start_with(input, options).await?;
    let outcome = interaction.result().await?;
    println!("Run: {} ({:?})", outcome.run.id, outcome.status);

    let mut content = client.api().assets().at(&asset.data.id).content()
        .get(Default::default()).await?;
    let mut downloaded = Vec::new();
    while let Some(chunk) = content.chunk().await? {
        downloaded.extend_from_slice(&chunk);
    }
    println!("Downloaded Asset matches source: {}", downloaded == original);
    Ok(())
}
```

Run `cargo +1.97.0 check` before using the real Service. The output includes a conditional Memory-file update's HTTP status, the exact Run status, and whether the downloaded Asset matches `report.txt`. A successful file round trip does **not** guarantee that a Model understands every media type; inspect the Run's outcome and committed Items. `BinaryResponse::chunk()` streams the response; do not buffer an arbitrarily large Asset as this demonstration does. Drop or `close()` an abandoned binary body.

Memory files use **logical paths** such as `notes.md`, not paths on your local disk. The example GETs `notes.md`, takes its exact ETag and uses `MemoryFileReplaceOptions.if_match` when replacing it. Delete similarly takes `MemoryFileDeleteOptions.if_match`; on `412`, read current content rather than overwriting it. A Run snapshots its Memory mounts at creation: changing a Thread's mount later will not retroactively change that Run. Record-provider Memory uses a provider-backed type and separate record/search APIs; it is not a binary Asset or a text file.

## Reuse Memory Spaces across Sessions

A Memory Space is a reusable workspace resource with its own managed file Memory. Create one through `client.api().memory_spaces().create(&models::MemorySpaceCreate::new(name), ...)`; use its `memory_id` with the existing file/revision graph. Select the **Space ID** through `SessionMemorySettings.spaces` and `SpaceBinding`, rather than putting the managed Memory ID in `StartOptions.memories`.

Session create omission or `memory: null` allows first successful business-Run acceptance to inherit Agent defaults. An explicit `enabled: false` disables participation; `spaces: []` explicitly clears selection. Session PATCH merges provided settings fields, replaces a provided spaces list, and treats outer `memory: null` as no change. Keep unset fields unset: default-filled readback is not a record of the caller's original intent.

This complete application creates a Space and Session and reads the frozen roots. It needs the connection settings and Agent ID above; the resulting resources remain in the disposable workspace.

```rust
use a13n::{Client, Secret, StartOptions, generated::models as m};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let space = client.api().memory_spaces().create(
        &m::MemorySpaceCreate::new("Project".into()), Default::default(),
    ).await?;
    let session = client.api().sessions().create(&m::SessionCreate {
        memory: Some(Some(Box::new(m::SessionMemorySettings {
            enabled: Some(true),
            spaces: Some(vec![m::SpaceBinding::new("project".into(), space.data.id)]),
        }))),
        ..Default::default()
    }, Default::default()).await?;
    let mut interaction = client.agent(env::var("A13N_AGENT_ID")?).start_with(
        "Read project notes", StartOptions {
            session_id: Some(Some(session.data.id)), ..Default::default()
        },
    ).await?;
    let outcome = interaction.result().await?;
    let run = client.run(&outcome.run_id).get().await?;
    for mount in run.data.memory_mounts {
        println!("{}: space={:?}, roots={:?}", mount.name, mount.space_id, mount.roots);
    }
    Ok(())
}
```

`FrozenMemoryMount` reads both legacy mounts and resolved Space roots without dropping `space_id` or `roots`. Ordinary business roots cover `global/` and the selected Session subtree; later Session edits do not change an accepted Run. Organization status/history are native `space.organization().get(...)` and `space.organizations().list(...)`. Manual `organizations().create(...)` returns **202 request status**, not completion; model/authority and organization scheduling belong to Service. Space deletion requires its ETag and no bound Session or unsealed frozen Run.

## Computer checks and retained screenshots

`client.api().environments().at(id).computer().check(...)` returns an instantaneous checked/unsupported/not_ready observation. It does not start a target, take a screenshot or grant Computer permissions. Computer calls require the Agent's computer toolset, explicit mount action permissions, provider support and readiness. An omitted ceiling preserves legacy file/execution actions with no Computer access; an explicit empty operations set grants nothing. `threads().at(thread).environments().at(name).update(...)` replaces the desired ceiling with the **Thread ETag** while existing Runs retain their frozen snapshot.

Committed Item `content.tool_images` contains retained image references. Fetch one explicitly with `client.api().runs().at(run).images().at(image).get(...)` and stream `BinaryResponse::chunk()` just like Asset content above. The response is raw PNG/JPEG and retains no-store and MIME headers; `contents()` remains the separate JSON/text API. Keep image metadata without automatically expanding it or treating a retained image ID as a fresh Computer observation handle. A failed display fetch never justifies replaying Computer input.

## Image and video references

A native `MessagePayload` preserves ordered text, `{ "type": "url", "url": "https://media.example/image.png" }` and `{ "type": "asset", "asset_id": "..." }` parts. URL parts do not take an SDK-specific media-type field. The same public Input works with `start_with`, `send_with` and atomic `ResumeOptions.input`. Model `HarnessModelCharacteristicsInput.image_input` and `video_input` describe supported preparation and URL policies; `ImageInputPolicy`, `VideoInputPolicy` and `UrlInputSupportInput` are generated typed wire models. For `image_input`, omission, explicit null and an explicit empty policy object remain different wire inputs; false and zero are not absence.

The SDK forwards references and characteristics rather than fetching or transcoding media. Service/Harness owns image preparation, video downloads, trusted TLS, host restrictions from the frozen Run configuration and byte budgets. An Asset round trip does not prove image/video Model support. Stream/Items media may be ordered structured descriptors with omitted payloads; retain metadata and inline-child attribution as described in [streams and results](streams-and-results.md#native-ag-ui-events-and-inline-child-attribution).

**Common mistakes:** Do not pass `upload_id` as a message's `asset_id`; publish an Asset first. Use the Agent ID rather than its name; keep upload and message request keys separate. A Run returning `waiting` still needs explicit action—see [waiting and resume](waiting-and-resume.md).
