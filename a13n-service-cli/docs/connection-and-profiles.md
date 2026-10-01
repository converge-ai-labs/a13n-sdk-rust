# Connect the CLI to an existing Service

Build the pre-public CLI [from source](../README.md#build-and-connect). You need a reachable Service URL, an authorized workspace API key and, for the conversation examples, an existing **Agent ID**. An API key selects its own workspace; ordinary calls do not take `--workspace`. A self-hosted Service with a private CA also needs a trusted PEM bundle.

## One-off connection

Use environment variables for a shell session, then check your resolved nonsecret configuration and a read-only Agent listing:

```bash
export A13N_BASE_URL='https://your-service.example'
export A13N_TOKEN='your-workspace-api-key'
a13n-service-cli config show
a13n-service-cli agents list --limit 10
```

`config show` does not print your token. `agents list` returns JSON by default; a `401` or `403` is a credential or permission problem, not proof that the target has no Agents. Do not commit the token or paste it into a `--body` argument.

## Save a named profile without storing credentials

For repeatable environments, make a JSON profile. This demonstration creates a **new isolated config file** rather than overwriting your existing `config.json`; merge the `dev` profile into that file yourself if you want to keep it. Replace the example URL and CA path with your actual Service values; omit `ca_bundle` when using public trusted roots:

```bash
mkdir -p "$HOME/.config/a13n-service-cli"
export A13N_CLI_CONFIG="$(mktemp "$HOME/.config/a13n-service-cli/guide.XXXXXX")"
cat > "$A13N_CLI_CONFIG" <<'JSON'
{
  "profiles": {
    "dev": {
      "base_url": "https://your-service.example",
      "ca_bundle": "/path/to/trusted-ca.pem",
      "token_env": "A13N_DEV_TOKEN"
    }
  }
}
JSON
export A13N_DEV_TOKEN='your-workspace-api-key'
a13n-service-cli --profile dev config show
a13n-service-cli --profile dev agents list --limit 10
```

`token_env` is an **environment-variable name**, never token bytes. `A13N_CLI_CONFIG` selects the isolated example; it remains a file until you remove it deliberately. Without that variable, the normal `config.json` remains unchanged. `--profile dev` ignores ambient URL/workspace/organization/CA settings; explicit CLI flags override the profile. Without `--profile`, precedence is explicit flags, environment variables, then config `defaults`. For a one-off private CA instead use `--ca-bundle /path/to/trusted-ca.pem`. The CLI has no insecure TLS switch.

**Login sessions are different:** `auth login` is a Service API call, not a persistent interactive CLI login or a stored cookie/CSRF session. The CLI's ordinary workflow uses an API key; use the SDK's session client if your application needs caller-owned cookies and CSRF. Public endpoints such as `healthz get` can be queried without a token. Explicit organization/workspace IDs are still required in administrative resource **paths**, and their authorization is Service-owned.

If a named profile appears to target the wrong Service, run `--profile dev config show` before sending a mutation. If TLS fails, fix the CA path rather than disabling verification; a successful local `--dry-run` never proves TLS or Service access. Next, [invoke an Agent](agent-workflows.md).

## Model Provider OAuth and discovery

All five operations use generated typed SDK dispatch, not a second OAuth/HTTP client. Read status or discover Models for an existing provider ID only when authorized (discovery can contact the external provider):

```bash
PROVIDER_ID='your-existing-model-provider-id'
a13n-service-cli model-providers authorization get --provider "$PROVIDER_ID"
a13n-service-cli model-providers models get --provider "$PROVIDER_ID"
a13n-service-cli model-providers authorize --schema
```

Manual authorization is available to workspace-write actors, including API keys. If a user explicitly asks to connect the provider, save initiation evidence privately and follow the returned `method`; do not assume a callback origin or client ID:

```bash
umask 077
printf '{"new_registration":false}\n' > authorization-request.json
a13n-service-cli model-providers authorize "$PROVIDER_ID" \
  --body @authorization-request.json > authorization-start.json
```

`browser_callback` requires an unconfined user login session and operator-configured hosted callback; an API-key CLI cannot perform that session-only flow. Use your application's authorized browser/session client rather than turning the entire provider group into a session-only API. For `manual_callback`, present the returned URL without logging its state. After user completion, create a protected `authorization-callback.json` containing the actual `attempt_id` and `callback_url` from that flow; callback codes are secrets, so do not paste them into command arguments or diagnostics:

```bash
a13n-service-cli model-providers authorization callback "$PROVIDER_ID" \
  --body @authorization-callback.json > authorization-status.json
```

Disconnect is a separate user-authorized mutation: `model-providers authorization delete --provider PROVIDER_ID`. Its `revocation_confirmed` may be null; local token removal is not proof of provider revocation. Service owns provider credentials and token storage. Mock/local dispatch checks do not establish real OAuth exchange; examples never hardcode a public callback endpoint.
