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
    let mut pages = client.api().agents().pages(AgentsListOptions {
        limit: Some(10), ..Default::default()
    });
    while let Some(page) = pages.next().await? {
        for agent in page.data.items {
            println!("Agent {}", agent.id);
        }
    }

    let id = env::var("A13N_DISPOSABLE_AGENT_ID")?;
    let agent = client.api().agents().at(&id); // Local binding; no HTTP call yet.
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
    let agents = client.api().agents().list(Default::default()).await?;
    println!("Session Agents on first page: {}", agents.data.items.len());
    Ok(())
}
```

`cargo +1.97.0 check` compiles without credentials; a real run prints the first page's size. Your login flow must rotate the shared CSRF value when it changes; the callback reads its current value on mutations, not a token cached by the SDK. Never combine this session with an API-key Bearer token, and never commit the cookie or CSRF token. The default workspace header is sent only to operations declaring it; an explicit per-operation header replaces the default.

## Choose credentials and scope deliberately

- **API key:** `Client::new(url, Secret::new(token))`; no workspace argument on ordinary Agent/Thread/Run calls. A key cannot select another workspace through `X-Workspace-ID`.
- **Login session:** use the cookie jar, live CSRF callback and explicit workspace shown above. This is not an alternative workspace-selection mechanism for an API key.
- **Public/admin operations:** health and other public calls need no token. Administrative operations carry their own declared path IDs and authorization; a workspace-scoped session default is not added to unrelated operations. For private TLS roots, configure `.http_builder(...)` and keep certificate verification enabled.

`client.api()` is the complete typed generated resource graph, for example `client.api().agents()`. Migrate older `client.resources()` calls to `client.api()`; no compatibility accessor remains. Authored `client.agent(id)` workflows are unchanged, as are `a13n::resources` option types and borrowed resource handles. `.at(id)` is only a local handle; an async leaf method performs a request. `Response<T>` keeps `data`, actual HTTP status and headers, ETag and request ID. A field typed `Option<Option<T>>` can be omitted (`None`), sent as JSON null (`Some(None)`) or sent with a value (`Some(Some(value))`), but the **Service** defines what null means for each field. The generated operations include the contract's status/headers, cursor pagination, binary ownership and administrative endpoints. `Client::execute` exposes the generated API functions on the same transport when you need an advanced direct call.

**Common mistakes:** Do not use an Agent name where an Agent ID is required; Models alone use keys. A session workspace header must not be copied into every request. Keep raw credentials and request bodies out of logs; see [errors and recovery](errors-and-recovery.md) for safe diagnostics.

## Model Provider OAuth and discovery

Use the canonical generated provider resource; it shares the same authentication, workspace scope and response metadata as other resources. This complete **read-only** example requires an existing OAuth provider ID in `A13N_MODEL_PROVIDER_ID` and an authorized workspace API key. Model discovery is a provider operation and can contact that provider, so compile offline first and run only when authorized:

```rust
use a13n::{Client, Secret};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let provider = client.api().model_providers().at(env::var("A13N_MODEL_PROVIDER_ID")?);
    let status = provider.authorization().get(Default::default()).await?;
    println!("Authorization state: {:?}, HTTP {}", status.data.state, status.status);
    let discovered = provider.models().get(Default::default()).await?;
    println!("Available models: {}", discovered.data.len());
    Ok(())
}
```

For an explicitly requested authorization, call `provider.authorize(&models::ProviderAuthorizationRequest { new_registration: Some(false) }, Default::default())`. Follow the returned `AuthorizationStart.method`, not a hardcoded client ID or callback origin. `browser_callback` requires an **unconfined user login session** using the caller-owned session setup above; hosted callback setup is an operator responsibility. Manual callback remains available to workspace-write actors, including API keys. Present the returned URL through your application's approved browser flow without logging its state.

Only after the user completes a manual flow, submit `models::AuthorizationCallback::new(attempt_id, callback_url)` to `provider.authorization().callback(...)` using that actual attempt and callback URL. Treat callback codes as secrets: never print the model or request body. Disconnect is an explicit mutation through `provider.authorization().delete(Default::default())`; its `revocation_confirmed: Option<Option<bool>>` can be null and must not be read as proof of remote revocation. The SDK stores no provider tokens and does not implement a second OAuth client. Local mock tests establish wire/status/scope behavior, not real user authorization or external token exchange.

## Inspect executed Runs and usage

`client.api().runs().list(RunsListOptions { session_id: Some(id), ..Default::default() })` searches exact workspace history. Filters also include Agent, Thread, principal, status and creation-time bounds; normal lazy pages preserve each response. `runs().at(run).agent().get(...)` returns the frozen revision and Agent config after frozen overrides, distinct from the current Agent revision and `Run.options.configuration`. Named models, connections and templates stay live.

`runs().at(run).attempts().at(attempt).get(...)` reads one authorized Attempt; the Attempt list is not paginated. Charged `RunView.attempts` may differ from the number of Attempt records. Required nullable fields remain present even when no start, finish or failure exists.

`client.api().usage().get(...)` includes per-Run model aggregates in `runs`. `usage().records().at(record).metrics().get(...)` reads one known model request's nullable TTFC, complete request duration and output tokens/sec. Zero is a measured zero; null means no eligible timing. Service computes speed over full request time, including first-chunk wait and non-streaming calls, and owns aggregation and late usage. There is no SDK-side metrics calculation or native record-list discovery operation.

## Configure tool discovery and read context usage

Tool search presents already selected sources through `ToolSearchConfig`, a proxy-only configuration object. Omitted mode uses Service defaults; native mode and strategy fields are invalid. Service validates names, bounds and sources. An omitted/null Run override inherits the revision; a non-null block replaces it and empty groups disables grouping.

This complete example constructs an explicit proxy override and reads one existing Run's context contribution. Set `A13N_RUN_ID` in addition to the usual URL/token. The override is serialized locally for inspection; the read is the only remote operation.

```rust
use a13n::{Client, Secret, Overrides, ToolSearch, ToolSearchConfig};
use a13n::resources::UsageContextGetOptions;
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let search = ToolSearch {
        config: Some(Box::new(ToolSearchConfig {
            max_results: Some(7), ..Default::default()
        })),
        groups: Some(Default::default()),
    };
    let overrides = Overrides::new().tool_search(Some(search));
    assert_eq!(serde_json::to_value(overrides)?["tool_search"]["groups"], serde_json::json!({}));
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let response = client.api().usage().context().get(UsageContextGetOptions {
        run_id: Some(env::var("A13N_RUN_ID")?), ..Default::default()
    }).await?;
    println!("HTTP {}, request {:?}", response.status, response.request_id());
    match response.data {
        None => println!("No completed model contribution"),
        Some(usage) => println!("Run {} record {:?}: input {:?}, output {:?}, window {:?}",
            usage.run_id, usage.record_id, usage.input_tokens, usage.output_tokens, usage.context_window_tokens),
    }
    Ok(())
}
```

Add `serde_json = "1"` to the source-installed application. Supply exactly one of `thread_id`, `run_id` or `record_id`; Service rejects missing/multiple selectors. Thread/Run selects the latest eligible completed root-agent generation, not a pending prompt estimate or aggregate. A successful JSON null means no matching contribution; object token/window fields remain independently nullable and zero stays zero. Unknown/missing resources remain API errors. The SDK derives no ratio, billing total or synthetic context capacity.

## Browse bounded run history

Run lists return `RunListItem` metadata with `input_preview.text` (at most 200 Unicode code points) and `input_preview.truncated`. Fetch an individual Run for full input, output, options or memory mounts. List `view` selection is removed.

Thread runs default to newest first. `ThreadRunPage` exposes `total`, `previous_cursor` (newer) and `next_cursor` (older). Each `ThreadRunItem` carries its chronological `number`, nullable decimal-string `cost`, `model_calls` and `unpriced_requests`; use the item's `number` instead of the removed page-level `first_number`. Use `anchor` for the first page around an exact Run; subsequent requests send the returned cursor without the anchor. Alternatively, `sort=cost|calls|duration` with `direction=asc|desc` ranks the whole Thread before pagination and cannot be combined with `anchor`. Ranking has no previous cursor. Reads and page iterators are lazy.

Session lists support `sort=updated_at|runs|cost` and `direction=asc|desc`. Session views require `run_count`, nullable decimal-string `cost` and `unpriced_requests`. Unknown cost remains null, distinct from measured zero. Session streams also expose native schedule and occurrence frames.
