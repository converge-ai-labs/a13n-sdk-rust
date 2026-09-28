# Attach a file and mount Memory

Assets are binary uploads; Memory files are editable text held in a Memory. An upload ID alone is **not** an Asset ID: publish the upload as an Asset before adding it to a message. To make Memory available to a Run, mount the Memory ID when starting the Thread.

You need the [source-installed SDK](../README.md#get-started-from-source), Service URL, workspace API key, existing Agent ID, two distinct keys (`A13N_UPLOAD_KEY` and `A13N_REQUEST_KEY`), and a local UTF-8 file to attach. From the sibling application's directory, create one with `printf 'Project notes\n' > report.txt`. The example creates an upload, Asset and Memory on the Service; use disposable state and a Model capable of working with your chosen attachment.

```rust
use a13n::{Client, Secret, StartOptions, UploadFile, generated::models, resources::MemoryFileReplaceOptions, text_payload};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let original = std::fs::read("report.txt")?;
    let upload = client.resources().uploads().create(
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
    let asset = client.resources().assets().create(
        &models::AssetCreate::new("Project report".into(), upload.data.upload_id),
        Default::default(),
    ).await?;

    let memory = client.resources().memories().create(
        &models::MemoryCreate::new("Project notes".into()), Default::default(),
    ).await?;
    client.resources().memories().at(&memory.data.id).files().create(
        &models::MemoryFileCreate::new("Read the project report.".into(), "notes.md".into()),
        Default::default(),
    ).await?;
    let file = client.resources().memories().at(&memory.data.id).files().at("notes.md");
    let current = file.get(Default::default()).await?;
    let etag = current.etag().ok_or("Memory file has no ETag")?.to_owned();
    let updated = file.replace(
        &models::MemoryFileReplace::new(format!("{}\nRecord open questions.", current.data.content)),
        MemoryFileReplaceOptions { if_match: etag, ..Default::default() },
    ).await?;
    println!("Memory file updated: HTTP {}", updated.status);

    let mut input = text_payload("Summarize the attached report using project notes.");
    input.content.push(models::Part::Asset(Box::new(models::AssetPart::new(
        asset.data.id.clone(), models::asset_part::Type::Asset,
    ))));
    let options = StartOptions {
        memories: Some(vec![models::MemoryMount::new(
            models::MemoryAccess::Read, memory.data.id, "notes".into(),
        )]),
        ..Default::default()
    };
    let mut interaction = client.agent(env::var("A13N_AGENT_ID")?)
        .start_with(input, env::var("A13N_REQUEST_KEY")?, options).await?;
    let outcome = interaction.result().await?;
    println!("Run: {} ({:?})", outcome.run.id, outcome.status());

    let mut content = client.resources().assets().at(&asset.data.id).content()
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

**Common mistakes:** Do not pass `upload_id` as a message's `asset_id`; publish an Asset first. Use the Agent ID rather than its name; keep upload and message request keys separate. A Run returning `waiting` still needs explicit action—see [waiting and resume](waiting-and-resume.md).
