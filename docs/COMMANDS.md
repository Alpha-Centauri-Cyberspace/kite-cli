# Kite CLI Command Reference

This file is auto-generated from the live `clap` help output.
Do not edit manually. Run:

```bash
./scripts/generate-cli-docs.sh
```

## Payload Output Modes

- `kite stream` (default) prints concise summary lines.
- `kite stream --compact` prints summary-only one-liners.
- `kite stream --json` prints full CloudEvent payload JSON.

## `kite`

```text
Universal webhook adapter CLI

Usage: kite [OPTIONS] [COMMAND]

Commands:
  stream     Stream webhook events to stdout
  proxy      Proxy webhook events to a local HTTP server
  listen     Listen for events via Unix socket or exec
  run        Run with a kite.json manifest
  retry      Retry failed events from the dead letter queue
  login      Log in to Kite via device auth flow
  github     Install GitHub webhooks into repositories using local gh auth
  endpoints  Manage webhook endpoints
  keys       Manage API keys
  skill      Install and inspect agent skills
  logs       View persisted event logs
  status     Show dashboard-equivalent status summary
  update     Update the installed kite binary
  queue      Inspect and manage the local event queue
  agent      Send and receive agent-to-agent messages on Kite Cloud
  help       Print this message or the help of the given subcommand(s)

Options:
      --install-skill <NAME>  Compatibility shortcut for `kite skill install <NAME>`
  -h, --help                  Print help
  -V, --version               Print version

Quick Reference:
  stream               --source --event-type --json --compact
  proxy                --source --target --route
  listen               --socket --source
  run                  --manifest
  retry                --source --target
  github install       --repo --events --all-events --rotate-secret
  login                --server
  endpoints create     --source
  endpoints deactivate --id
  keys create          --name --scopes --permissions --expires-at
  keys revoke          --id
  skill install        <name>
  skill list
  logs                 --limit
  update               --check --force --server

Use `kite stream --json` to print full CloudEvent payloads.
Use `kite <command> --help` for complete options.
```

## `kite stream`

```text
Stream webhook events to stdout

Usage: kite stream [OPTIONS]

Options:
      --source <SOURCE>           Filter by source (e.g. "github", "stripe")
      --event-type <type>         Filter by event type
      --json                      Output full CloudEvent JSON per line
      --compact                   Output compact one-line summaries only
      --exec <EXEC>               Execute a command for each event, passing event JSON on stdin
      --client-id <CLIENT_ID>     Persistent client ID for delivery cursor tracking
      --importance <IMPORTANCE>   Minimum importance level to deliver (low, normal, high, critical)
  -h, --help                      Print help
  -V, --version                   Print version
```

## `kite proxy`

```text
Proxy webhook events to a local HTTP server

Usage: kite proxy [OPTIONS]

Options:
      --source <SOURCE>        Filter by source (e.g. "github", "stripe")
      --target <TARGET>        Default target URL to forward events to
      --route <SOURCE=TARGET>  Source-specific route in the form <source>=<target> (repeatable)
      --client-id <CLIENT_ID>  Persistent client ID for delivery cursor tracking
  -h, --help                   Print help
  -V, --version                Print version

Examples:
  # Single target mode: all sources -> one local endpoint
  kite proxy --target http://localhost:3000/webhooks

  # Multi-route mode with fallback/default target
  kite proxy --route github=http://localhost:3001/github \
             --route stripe=http://localhost:3002/stripe \
             --target http://localhost:3000/default

  # Route-only mode (no fallback): errors + DLQ when source has no explicit route
  kite proxy --route github=http://localhost:3001/github

  # Downstream router mode: keep one local endpoint and dispatch by headers
  kite proxy --target http://localhost:3000/kite/router
  # Forwarded metadata headers include:
  #   x-kite-source, x-kite-event-id, x-kite-event-type, x-kite-team-id
  #   ce-id, ce-source, ce-type, ce-specversion, ce-time (if present)

Notes:
  Existing Kite webhook endpoints do not need to be recreated to use routes.
  --route keys are matched by source name (derived from event type, e.g. com.github.push -> github).
```

## `kite listen`

```text
Listen for events via Unix socket or exec

Usage: kite listen [OPTIONS]

Options:
      --socket <SOCKET>  Unix socket path to create
      --source <SOURCE>  Filter by source
  -h, --help             Print help
  -V, --version          Print version
```

## `kite run`

```text
Run with a kite.json manifest

Usage: kite run --manifest <MANIFEST>

Options:
      --manifest <MANIFEST>  Path to kite.json manifest file
  -h, --help                 Print help
  -V, --version              Print version
```

## `kite retry`

```text
Retry failed events from the dead letter queue

Usage: kite retry [OPTIONS] --target <TARGET>

Options:
      --source <SOURCE>  Only retry events from this source
      --target <TARGET>  Target URL for retried events
  -h, --help             Print help
  -V, --version          Print version
```

## `kite login`

```text
Log in to Kite via device auth flow

Usage: kite login [OPTIONS]

Options:
      --server <SERVER>  Server URL [default: https://getkite.sh]
  -h, --help             Print help
  -V, --version          Print version
```

## `kite github`

```text
Install GitHub webhooks into repositories using local gh auth

Usage: kite github <COMMAND>

Commands:
  install  Create or update a GitHub repository webhook that points at Kite
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite github install`

```text
Create or update a GitHub repository webhook that points at Kite

Usage: kite github install [OPTIONS] --repo <REPO>

Options:
      --repo <REPO>      Repository in OWNER/REPO format
      --events <EVENTS>  Comma-separated GitHub event names that replace the default bundle
      --all-events       Subscribe the webhook to all GitHub events
      --rotate-secret    Rotate the stored GitHub webhook secret while reinstalling
  -h, --help             Print help
  -V, --version          Print version

Examples:
  # Install the default agent-focused GitHub event bundle
  kite github install --repo owner/repo

  # Replace defaults with an explicit event list
  kite github install --repo owner/repo --events push,pull_request

  # Subscribe to every GitHub event
  kite github install --repo owner/repo --all-events

  # Rotate the stored GitHub secret while reinstalling the hook
  kite github install --repo owner/repo --rotate-secret

Default events:
  push,pull_request,issues,issue_comment,pull_request_review,pull_request_review_comment

Notes:
  --events replaces the default bundle entirely.
  --all-events is mutually exclusive with --events.
  This command uses your local `gh` authentication and updates the GitHub webhook in-place when rerun.
```

## `kite endpoints`

```text
Manage webhook endpoints

Usage: kite endpoints <COMMAND>

Commands:
  list        List endpoints
  create      Create or rotate endpoint credentials for a source (GitHub includes a one-time webhook secret)
  deactivate  Deactivate endpoint by id
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite endpoints list`

```text
List endpoints

Usage: kite endpoints list

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite endpoints create`

```text
Create or rotate endpoint credentials for a source (GitHub includes a one-time webhook secret)

Usage: kite endpoints create [OPTIONS] --source <SOURCE>

Options:
      --source <SOURCE>                  Source name (e.g. github, stripe)
      --repo <REPO>                      GitHub repo (owner/name) — auto-registers webhook via GitHub API
      --events <EVENTS>                  Webhook events to subscribe to (comma-separated, default: push,pull_request,issues)
      --force                            Replace existing webhook on the repo
      --github-token <GITHUB_TOKEN>      GitHub personal access token (falls back to GITHUB_TOKEN env or gh CLI)
      --signing-secret <SIGNING_SECRET>  Provider-issued signing secret (e.g. Linear's lin_wh_…). Use `-` to read from stdin
  -h, --help                             Print help
  -V, --version                          Print version
```

## `kite endpoints deactivate`

```text
Deactivate endpoint by id

Usage: kite endpoints deactivate --id <ID>

Options:
      --id <ID>  Endpoint id
  -h, --help     Print help
  -V, --version  Print version
```

## `kite keys`

```text
Manage API keys

Usage: kite keys <COMMAND>

Commands:
  list    List API keys
  create  Create a new API key
  revoke  Revoke API key by id
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite keys list`

```text
List API keys

Usage: kite keys list

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite keys create`

```text
Create a new API key

Usage: kite keys create [OPTIONS]

Options:
      --name <NAME>                Optional key name
      --scopes <SCOPES>            Comma-separated scopes
      --permissions <PERMISSIONS>  Comma-separated permissions
      --expires-at <EXPIRES_AT>    Optional RFC3339 expiry timestamp
  -h, --help                       Print help
  -V, --version                    Print version
```

## `kite keys revoke`

```text
Revoke API key by id

Usage: kite keys revoke --id <ID>

Options:
      --id <ID>  Key id
  -h, --help     Print help
  -V, --version  Print version
```

## `kite skill`

```text
Install and inspect agent skills

Usage: kite skill <COMMAND>

Commands:
  install           Install a skill from the default registry
  list              List installed skills
  export            Export the kite SKILL.md to agent skill directories
  publish           Publish a skill to the central registry
  search            Search for skills in the registry
  registry-install  Install a skill from the remote registry by ID
  help              Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite skill install`

```text
Install a skill from the default registry

Usage: kite skill install <NAME>

Arguments:
  <NAME>  Skill name (e.g. weather, github)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite skill list`

```text
List installed skills

Usage: kite skill list

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite skill publish`

```text
Publish a skill to the central registry

Usage: kite skill publish [OPTIONS] --name <NAME> --version <VERSION>

Options:
      --name <NAME>                Skill name (alphanumeric, hyphens, underscores, dots)
      --version <VERSION>          Semver version string (e.g. "1.0.0")
      --description <DESCRIPTION>  Short description of the skill
      --content <CONTENT>          Skill content (reads SKILL.md from current directory if omitted)
      --public                     Make the skill publicly discoverable
  -h, --help                       Print help
```

## `kite skill search`

```text
Search for skills in the registry

Usage: kite skill search [QUERY]

Arguments:
  [QUERY]  Search query (matches name and description)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite skill registry-install`

```text
Install a skill from the remote registry by ID

Usage: kite skill registry-install [OPTIONS] --skill-id <SKILL_ID>

Options:
      --skill-id <SKILL_ID>      Skill ID (UUID) to install
      --version-id <VERSION_ID>  Specific version ID to install (latest if omitted)
  -h, --help                     Print help
  -V, --version                  Print version
```

## `kite logs`

```text
View persisted event logs

Usage: kite logs [OPTIONS]

Options:
      --limit <LIMIT>  Number of events to fetch [default: 100]
  -h, --help           Print help
  -V, --version        Print version
```

## `kite status`

```text
Show dashboard-equivalent status summary

Usage: kite status

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite update`

```text
Update the installed kite binary

Usage: kite update [OPTIONS]

Options:
      --check            Check whether an update is available without installing it
      --force            Force reinstall even if current version is up to date
      --server <SERVER>  Override update server base URL
  -h, --help             Print help
  -V, --version          Print version
```

## `kite agent`

```text
Send and receive agent-to-agent messages on Kite Cloud

Usage: kite agent <COMMAND>

Commands:
  register  Register a stable agent identity on this machine
  listen    Listen for `com.kite.agent.message` events addressed to this agent
  send      Send an agent message to another agent on the same team
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `kite agent register`

```text
Register a stable agent identity on this machine

Usage: kite agent register [OPTIONS]

Options:
      --name <NAME>  Optional friendly name (slugged into the agent id)
  -h, --help         Print help
  -V, --version      Print version
```

## `kite agent listen`

```text
Listen for `com.kite.agent.message` events addressed to this agent

Usage: kite agent listen [OPTIONS]

Options:
      --as-id <AGENT_ID>  Override the agent id to listen as (defaults to the registered id)
      --json              Print full CloudEvent JSON per line instead of a one-line summary
  -h, --help              Print help
  -V, --version           Print version
```

## `kite agent send`

```text
Send an agent message to another agent on the same team

Usage: kite agent send [OPTIONS] --to <AGENT_ID> <BODY>

Arguments:
  <BODY>  Message body

Options:
      --to <AGENT_ID>          Recipient agent id
      --from <AGENT_ID>        Sender agent id (defaults to the registered id on this machine)
      --thread <THREAD_ID>     Optional thread id for grouping replies
      --reply-to <EVENT_ID>    Optional event id this message replies to
  -h, --help                   Print help
  -V, --version                Print version
```

