# Authenticate and work with resources

A workspace API key is the simplest way to call flat business resources. It selects its workspace automatically. Use a login session only when your application owns the cookie jar, current CSRF token and explicit workspace selection. Administrative URLs still take their own organization or workspace path IDs; a key's workspace does not grant administrative authority.

For this example, use the [source-installed application](../README.md#get-started-from-source), `A13N_SERVICE_URL`, `A13N_API_TOKEN` and the ID of a **disposable** Agent in `A13N_DISPOSABLE_AGENT_ID`. It reads Agent pages, then conditionally changes that Agent's description on the Service. Do not point it at a shared Agent.

```rust
use a13n::{Client, Secret, generated::models, resources::{AgentUpdateOptions, AgentsListOptions}};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let mut pages = client.resources().agents().pages(AgentsListOptions {
        limit: Some(10), ..Default::default()
    });
    while let Some(page) = pages.next().await? {
        for agent in page.data.items {
            println!("Agent {}", agent.id);
        }
    }

    let id = env::var("A13N_DISPOSABLE_AGENT_ID")?;
    let agent = client.resources().agents().at(&id); // Local binding; no HTTP call yet.
    let current = agent.get(Default::default()).await?;
    let etag = current.etag().ok_or("Agent response has no ETag")?;
    let change = models::AgentUpdate {
        description: Some(Some("Updated with an ETag".into())),
        ..Default::default()
    };
    let updated = agent.update(&change, AgentUpdateOptions {
        if_match: etag.into(), ..Default::default()
    }).await?;
    println!("Updated {}: HTTP {}, request {:?}", id, updated.status, updated.request_id());
    Ok(())
}
```

`cargo +1.97.0 check` verifies the types offline. With authorized disposable state, the program lists pages one at a time and prints the real update status/request ID. `.pages(options).next().await?` is lazy: it does not fetch all pages upfront and refuses a repeated cursor. The update uses **the exact ETag from the preceding GET**. A `412` means someone else changed the resource; read the latest state and reconcile instead of copying a new ETag into a blind retry.

## Use a login session only when your application owns it

A session client needs a cookie jar populated by a real login, a callback for the **current** CSRF token, and a workspace ID. For an application that already acquired an authorized cookie and CSRF token, add `reqwest = { version = "0.13", default-features = false, features = ["cookies", "rustls"] }` to `Cargo.toml`. Set `A13N_SESSION_COOKIE` to the acquired `name=value` cookie (not a Bearer token), `A13N_CSRF_TOKEN` to its current CSRF token, and `A13N_WORKSPACE_ID` to its selected workspace. This separate, complete example lists Agents in that session:

```rust
use a13n::Client;
use std::{env, error::Error, sync::{Arc, RwLock}};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let jar = Arc::new(reqwest::cookie::Jar::default());
    jar.add_cookie_str(&env::var("A13N_SESSION_COOKIE")?, &reqwest::Url::parse(&url)?);
    let csrf = Arc::new(RwLock::new(env::var("A13N_CSRF_TOKEN")?));
    let current_csrf = Arc::clone(&csrf);
    let client = Client::builder(&url)
        .session(jar, move || current_csrf.read().ok().map(|value| value.clone()))
        .workspace(env::var("A13N_WORKSPACE_ID")?)
        .build()?;
    let agents = client.resources().agents().list(Default::default()).await?;
    println!("Session Agents on first page: {}", agents.data.items.len());
    Ok(())
}
```

`cargo +1.97.0 check` compiles without credentials; a real run prints the first page's size. Your login flow must rotate the shared CSRF value when it changes; the callback reads its current value on mutations, not a token cached by the SDK. Never combine this session with an API-key Bearer token, and never commit the cookie or CSRF token. The default workspace header is sent only to operations declaring it; an explicit per-operation header replaces the default.

## Choose credentials and scope deliberately

- **API key:** `Client::new(url, Secret::new(token))`; no workspace argument on ordinary Agent/Thread/Run calls. A key cannot select another workspace through `X-Workspace-ID`.
- **Login session:** use the cookie jar, live CSRF callback and explicit workspace shown above. This is not an alternative workspace-selection mechanism for an API key.
- **Public/admin operations:** health and other public calls need no token. Administrative operations carry their own declared path IDs and authorization; a workspace-scoped session default is not added to unrelated operations. For private TLS roots, configure `.http_builder(...)` and keep certificate verification enabled.

`client.resources()` is the complete typed generated resource graph. `.at(id)` is only a local handle; an async leaf method performs a request. `Response<T>` keeps `data`, actual HTTP status and headers, ETag and request ID. A field typed `Option<Option<T>>` can be omitted (`None`), sent as JSON null (`Some(None)`) or sent with a value (`Some(Some(value))`), but the **Service** defines what null means for each field. The generated operations include the contract's status/headers, cursor pagination, binary ownership and administrative endpoints. `Client::execute` exposes the generated API functions on the same transport when you need an advanced direct call.

**Common mistakes:** Do not use an Agent name where an Agent ID is required; Models alone use keys. A session workspace header must not be copied into every request. Keep raw credentials and request bodies out of logs; see [errors and recovery](errors-and-recovery.md) for safe diagnostics.
