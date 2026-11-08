# Paseo CLI surface

## paseo
```
Usage: paseo [options] [command]

Paseo CLI - control your AI coding agents from the command line

Options:
  -v, --version                    output the version number
  -o, --format <format>            output format: table, json, yaml (default:
                                   "table")
  --json                           output in JSON format (alias for --format
                                   json)
  -q, --quiet                      minimal output (IDs only)
  --no-headers                     omit table headers
  --no-color                       disable colored output
  --host <host>                    Explicit daemon endpoint: host:port, TCP,
                                   socket, SSH, or pairing URL (default: local
                                   home)
  --home <path>                    Select a local daemon home (default:
                                   ~/.paseo)
  -h, --help                       display help for command

Commands:
  host [options]                   Manage hosts connected to the selected
                                   daemon
  ls [options]                     List agents. By default excludes archived
                                   agents.
  run [options] <prompt>           Create and start an agent with a task
  import [options] <id>            Import an existing provider session as a
                                   Paseo agent
  clone [options] <repo>           Clone a GitHub repo and register it as a
                                   Paseo workspace
  attach [options] <id>            Attach to a running agent's output stream
  logs [options] <id>              View agent activity/timeline
  stop [options] [id]              Interrupt an agent if it is running (no-op
                                   for idle agents)
  delete [options] [id]            Delete an agent (interrupt if running, then
                                   hard-delete)
  send [options] <id> [prompt]     Send a message/task to an existing agent
  inspect [options] <id>           Show detailed information about an agent
  wait [options] <id>              Wait for an agent to become idle
  archive [options] <id>           Archive an agent (soft-delete)
  onboard [options]                Run first-time setup, start daemon, and
                                   print pairing instructions
  start [options]                  Start the local daemon from persistent
                                   configuration (local operation)
  hooks [options] <agent> <event>  Record agent hook activity
  status [options]                 Observe the selected daemon and its
                                   published endpoint
  restart [options]                Restart the selected daemon worker,
                                   retaining its supervisor launch
  reload [options]                 Reload config.json without restarting
  pair [options]                   Print the daemon pairing QR code and link
  agent [options]                  Manage agents (advanced operations)
  daemon [options]                 Manage the Paseo daemon
  hub [options]                    Manage Paseo Hub
  terminal [options]               Manage workspace terminals
  script [options]                 Manage configured workspace scripts
  schedule [options]               Manage recurring schedules
  heartbeat [options]              Manage this agent's heartbeats
  permit [options]                 Manage permission requests
  provider [options]               Manage agent providers
  usage [options]                  Show account usage reports
  plugin [options]                 Manage trusted, unsandboxed plugins
  speech [options]                 Speech commands
  project [options]                Manage projects
  workspace [options]              Manage workspaces
  help [command]                   display help for command

```

## paseo host — Manage hosts connected to the selected
```
Usage: paseo host [options] [command]

Manage hosts connected to the selected daemon

Options:
  -o, --format <format>                output format: table, json, yaml
                                       (default: "table")
  --json                               output in JSON format (alias for
                                       --format json)
  -q, --quiet                          minimal output (IDs only)
  --no-headers                         omit table headers
  --no-color                           disable colored output
  --host <host>                        Explicit daemon endpoint: host:port,
                                       TCP, socket, SSH, or pairing URL
                                       (default: local home)
  --home <path>                        Select a local daemon home (default:
                                       ~/.paseo)
  -v, --version                        output the version number
  -h, --help                           display help for command

Commands:
  ls [options]                         List saved hosts, including disabled and
                                       failed entries
  add [options] <connection>
  update [options] <registration-id>
  inspect [options] <registration-id>
  enable [options] <registration-id>
  disable [options] <registration-id>
  rm [options] <registration-id>
  help [command]                       display help for command

```

### paseo host ls — List saved hosts, including disabled and
```
Usage: paseo host ls [options]

List saved hosts, including disabled and failed entries

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

## paseo ls — List agents. By default excludes archived
```
Usage: paseo ls [options]

List agents. By default excludes archived agents.

Options:
  -a, --all              Include archived and background agents
  -g, --global           List agents across all directories
  --background           Include background workspace agents
  --label <key=value>    Filter by label (can be used multiple times) (default:
                         [])
  --thinking <id>        Filter by thinking option ID
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo run — Create and start an agent with a task
```
Usage: paseo run [options] <prompt>

Create and start an agent with a task

Arguments:
  prompt                            The task/prompt for the agent

Options:
  --no-wait                         Return immediately without waiting for
                                    completion
  --title <title>                   Assign a title to the agent
  --provider <provider>             Agent provider, or provider/model (e.g.
                                    codex or codex/gpt-5.4)
  --model <model>                   Model to use (e.g.,
                                    claude-sonnet-4-20250514,
                                    claude-3-5-haiku-20241022)
  --thinking <id>                   Thinking option ID to use for this run
  --mode <mode>                     Provider-specific mode (e.g., plan,
                                    default, bypass)
  --new-workspace <local|worktree>  Create a separate local or worktree
                                    workspace
  --worktree-mode <mode>            Worktree mode: branch-off, checkout-branch,
                                    or checkout-pr
  --worktree-slug <slug>            Managed worktree path slug
  --new-branch <name>               New branch name for branch-off mode
  --base <ref>                      Base ref for branch-off mode
  --branch <name>                   Existing branch for checkout-branch mode
  --pr-number <n>                   Pull request or change request number for
                                    checkout-pr mode
  --forge <forge>                   Forge for checkout-pr mode
  --workspace <id>                  Run in an existing workspace (defaults to
                                    the caller workspace when agent-scoped)
  --image <path>                    Attach image(s) to the initial prompt (can
                                    be used multiple times) (default: [])
  --cwd <path>                      Working directory (default: current)
  --env <key=value>                 Set environment variable(s) for the agent
                                    process (can be used multiple times)
                                    (default: [])
  --label <key=value>               Add label(s) to the agent (can be used
                                    multiple times) (default: [])
  --wait-timeout <duration>         Maximum time to wait for agent to finish
                                    (e.g., 30s, 5m, 1h). Default: no limit
  --output-schema <schema>          Outp
```

## paseo import — Import an existing provider session as a
```
Usage: paseo import [options] <id>

Import an existing provider session as a Paseo agent

Arguments:
  id                     Provider session/thread ID to import

Options:
  --provider <provider>  Agent provider id
  --cwd <path>           Working directory for providers that require it
  --label <key=value>    Add label(s) to the agent (can be used multiple times)
                         (default: [])
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo clone — Clone a GitHub repo and register it as a
```
Usage: paseo clone [options] <repo>

Clone a GitHub repo and register it as a Paseo workspace

Arguments:
  repo                   GitHub repo in owner/repo format or a full git remote
                         URL

Options:
  --dir <path>           Parent directory to clone into (for example:
                         ~/workspace)
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  --protocol <protocol>  Protocol for owner/repo shorthand repositories
                         (choices: "https", "ssh")
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo attach — Attach to a running agent's output stream
```
Usage: paseo attach [options] <id>

Attach to a running agent's output stream

Arguments:
  id                     Agent ID (or prefix)

Options:
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  --json                 output in JSON format (alias for --format json)
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo logs — View agent activity/timeline
```
Usage: paseo logs [options] <id>

View agent activity/timeline

Arguments:
  id                     Agent ID (or prefix)

Options:
  -f, --follow           Follow log output (streaming)
  --tail <n>             Show last n entries
  --filter <type>        Filter by event type (tools, text, errors,
                         permissions)
  --since <time>         Show logs since timestamp
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  --json                 output in JSON format (alias for --format json)
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo inspect — Show detailed information about an agent
```
Usage: paseo inspect [options] <id>

Show detailed information about an agent

Arguments:
  id                     Agent ID (or prefix)

Options:
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo wait — Wait for an agent to become idle
```
Usage: paseo wait [options] <id>

Wait for an agent to become idle

Arguments:
  id                     Agent ID (or prefix)

Options:
  --timeout <seconds>    Maximum wait time (default: no limit)
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo archive — Archive an agent (soft-delete)
```
Usage: paseo archive [options] <id>

Archive an agent (soft-delete)

Arguments:
  id                     Agent ID, prefix, or name

Options:
  --force                Force archive running agent (interrupts active run
                         first)
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo onboard — Run first-time setup, start daemon, and
```
Usage: paseo onboard [options]

Run first-time setup, start daemon, and print pairing instructions

Options:
  --home <path>          Local daemon home (default: ~/.paseo)
  --listen <listen>      Listen target (host:port, port, or unix socket path)
  --port <port>          Port to listen on (default: 6767)
  --relay                Enable relay connection without prompting
  --no-relay             Disable relay connection
  --no-mcp               Disable the Agent MCP HTTP endpoint
  --hostnames <hosts>    Daemon hostnames (comma-separated, e.g.
                         "myhost,.example.com" or "true" for any)
  --timeout <seconds>    Max time to wait for daemon readiness (default: 600)
  --voice <mode>         Voice setup mode: ask, enable, disable (default:
                         "ask")
  -o, --format <format>  output format: table, json, yaml (default: "table")
  --json                 output in JSON format (alias for --format json)
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo start — Start the local daemon from persistent
```
Usage: paseo start [options]

Start the local daemon from persistent configuration (local operation)

Options:
  --home <path>          Local daemon home (default: ~/.paseo)
  --timeout <seconds>    Readiness deadline (default: 600)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  --json                 output in JSON format (alias for --format json)
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo hooks — Record agent hook activity
```
Usage: paseo hooks [options] <agent> <event>

Record agent hook activity

Arguments:
  agent                  Agent hook source
  event                  Agent hook event

Options:
  -o, --format <format>  output format: table, json, yaml (default: "table")
  --json                 output in JSON format (alias for --format json)
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo status — Observe the selected daemon and its
```
Usage: paseo status [options]

Observe the selected daemon and its published endpoint

Options:
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo restart — Restart the selected daemon worker,
```
Usage: paseo restart [options]

Restart the selected daemon worker, retaining its supervisor launch

Options:
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  --timeout <seconds>    Replacement readiness deadline (default: 600)
  --kill-terminals       Close every terminal instead of keeping them across
                         the restart
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo reload — Reload config.json without restarting
```
Usage: paseo reload [options]

Reload config.json without restarting

Options:
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo pair — Print the daemon pairing QR code and link
```
Usage: paseo pair [options]

Print the daemon pairing QR code and link

Options:
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  --relay                Enable relay without prompting
  -o, --format <format>  output format: table, json, yaml (default: "table")
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo agent — Manage agents (advanced operations)
```
Usage: paseo agent [options] [command]

Manage agents (advanced operations)

Options:
  -o, --format <format>         output format: table, json, yaml (default:
                                "table")
  --json                        output in JSON format (alias for --format json)
  -q, --quiet                   minimal output (IDs only)
  --no-headers                  omit table headers
  --no-color                    disable colored output
  --host <host>                 Explicit daemon endpoint: host:port, TCP,
                                socket, SSH, or pairing URL (default: local
                                home)
  --home <path>                 Select a local daemon home (default: ~/.paseo)
  -v, --version                 output the version number
  -h, --help                    display help for command

Commands:
  ls [options]                  List agents. By default excludes archived
                                agents.
  run [options] <prompt>        Create and start an agent with a task
  import [options] <id>         Import an existing provider session as a Paseo
                                agent
  attach [options] <id>         Attach to a running agent's output stream
  logs [options] <id>           View agent activity/timeline
  open [options] <agent-id>     Open an existing agent in Paseo Desktop
  stop [options] [id]           Interrupt an agent if it is running (no-op for
                                idle agents)
  delete [options] [id]         Delete an agent (interrupt if running, then
                                hard-delete)
  send [options] <id> [prompt]  Send a message/task to an existing agent
  inspect [options] <id>        Show detailed information about an agent
  wait [options] <id>           Wait for an agent to become idle
  mode [options] <id> [mode]    Change an agent's operational mode
  archive [options] <id>        Archive an agent (soft-delete)
  reload [options] <id>         Reload an agent (restarts the underlying
                                process)
  detach [options] <id>         Make a subagent independent without stopping or
                                moving it
  update [options] <id>         Update an agent's settings or metadata
  help [command]                display help for command

```

### paseo agent ls — List agents. By default excludes archived
```
Usage: paseo agent ls [options]

List agents. By default excludes archived agents.

Options:
  -a, --all            Include archived and background agents
  -g, --global         List agents across all directories
  --background         Include background workspace agents
  --label <key=value>  Filter by label (can be used multiple times) (default:
                       [])
  --thinking <id>      Filter by thinking option ID
  --json               Output in JSON format
  --host <host>        Explicit daemon endpoint: host:port, TCP, socket, SSH,
                       or pairing URL (default: local home)
  --home <path>        Select a local daemon home (default: ~/.paseo)
  -h, --help           display help for command

```

### paseo agent run — Create and start an agent with a task
```
Usage: paseo agent run [options] <prompt>

Create and start an agent with a task

Arguments:
  prompt                            The task/prompt for the agent

Options:
  --no-wait                         Return immediately without waiting for
                                    completion
  --title <title>                   Assign a title to the agent
  --provider <provider>             Agent provider, or provider/model (e.g.
                                    codex or codex/gpt-5.4)
  --model <model>                   Model to use (e.g.,
                                    claude-sonnet-4-20250514,
                                    claude-3-5-haiku-20241022)
  --thinking <id>                   Thinking option ID to use for this run
  --mode <mode>                     Provider-specific mode (e.g., plan,
                                    default, bypass)
  --new-workspace <local|worktree>  Create a separate local or worktree
                                    workspace
  --worktree-mode <mode>            Worktree mode: branch-off, checkout-branch,
                                    or checkout-pr
  --worktree-slug <slug>            Managed worktree path slug
  --new-branch <name>               New branch name for branch-off mode
  --base <ref>                      Base ref for branch-off mode
  --branch <name>                   Existing branch for checkout-branch mode
  --pr-number <n>                   Pull request or change request number for
                                    checkout-pr mode
  --forge <forge>                   Forge for checkout-pr mode
  --workspace <id>                  Run in an existing workspace (defaults to
                                    the caller workspace when agent-scoped)
  --image <path>                    Attach image(s) to the initial prompt (can
                                    be used multiple times) (default: [])
  --cwd <path>                      Working directory (default: current)
  --env <key=value>         
```

### paseo agent import — Import an existing provider session as a Paseo
```
Usage: paseo agent import [options] <id>

Import an existing provider session as a Paseo agent

Arguments:
  id                     Provider session/thread ID to import

Options:
  --provider <provider>  Agent provider id
  --cwd <path>           Working directory for providers that require it
  --label <key=value>    Add label(s) to the agent (can be used multiple times)
                         (default: [])
  --json                 Output in JSON format
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -h, --help             display help for command

```

### paseo agent attach — Attach to a running agent's output stream
```
Usage: paseo agent attach [options] <id>

Attach to a running agent's output stream

Arguments:
  id             Agent ID (or prefix)

Options:
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo agent logs — View agent activity/timeline
```
Usage: paseo agent logs [options] <id>

View agent activity/timeline

Arguments:
  id               Agent ID (or prefix)

Options:
  -f, --follow     Follow log output (streaming)
  --tail <n>       Show last n entries
  --filter <type>  Filter by event type (tools, text, errors, permissions)
  --since <time>   Show logs since timestamp
  --host <host>    Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                   pairing URL (default: local home)
  --home <path>    Select a local daemon home (default: ~/.paseo)
  -h, --help       display help for command

```

### paseo agent open — Open an existing agent in Paseo Desktop
```
Usage: paseo agent open [options] <agent-id>

Open an existing agent in Paseo Desktop

Arguments:
  agent-id              Existing agent ID

Options:
  --server <server-id>  Server ID (defaults to the local daemon)
  --json                Output in JSON format
  --host <host>         Explicit daemon endpoint: host:port, TCP, socket, SSH,
                        or pairing URL (default: local home)
  --home <path>         Select a local daemon home (default: ~/.paseo)
  -h, --help            display help for command

```

### paseo agent inspect — Show detailed information about an agent
```
Usage: paseo agent inspect [options] <id>

Show detailed information about an agent

Arguments:
  id             Agent ID (or prefix)

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo agent wait — Wait for an agent to become idle
```
Usage: paseo agent wait [options] <id>

Wait for an agent to become idle

Arguments:
  id                   Agent ID (or prefix)

Options:
  --timeout <seconds>  Maximum wait time (default: no limit)
  --json               Output in JSON format
  --host <host>        Explicit daemon endpoint: host:port, TCP, socket, SSH,
                       or pairing URL (default: local home)
  --home <path>        Select a local daemon home (default: ~/.paseo)
  -h, --help           display help for command

```

### paseo agent archive — Archive an agent (soft-delete)
```
Usage: paseo agent archive [options] <id>

Archive an agent (soft-delete)

Arguments:
  id             Agent ID, prefix, or name

Options:
  --force        Force archive running agent (interrupts active run first)
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo agent reload — Reload an agent (restarts the underlying
```
Usage: paseo agent reload [options] <id>

Reload an agent (restarts the underlying process)

Arguments:
  id             Agent ID, prefix, or name

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo agent detach — Make a subagent independent without stopping or
```
Usage: paseo agent detach [options] <id>

Make a subagent independent without stopping or moving it

Arguments:
  id             Agent ID, prefix, or name

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo agent update — Update an agent's settings or metadata
```
Usage: paseo agent update [options] <id>

Update an agent's settings or metadata

Arguments:
  id               Agent ID (or prefix)

Options:
  --name <name>    Update the agent's display name
  --thinking <id>  Update the agent's thinking option ID
  --label <label>  Add/set label(s) on the agent (can be used multiple times or
                   comma-separated) (default: [])
  --json           Output in JSON format
  --host <host>    Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                   pairing URL (default: local home)
  --home <path>    Select a local daemon home (default: ~/.paseo)
  -h, --help       display help for command

```

## paseo daemon — Manage the Paseo daemon
```
Usage: paseo daemon [options] [command]

Manage the Paseo daemon

Options:
  -o, --format <format>   output format: table, json, yaml (default: "table")
  --json                  output in JSON format (alias for --format json)
  -q, --quiet             minimal output (IDs only)
  --no-headers            omit table headers
  --no-color              disable colored output
  --host <host>           Explicit daemon endpoint: host:port, TCP, socket,
                          SSH, or pairing URL (default: local home)
  --home <path>           Select a local daemon home (default: ~/.paseo)
  -v, --version           output the version number
  -h, --help              display help for command

Commands:
  start [options]         Start the local daemon from persistent configuration
                          (local operation)
  run [options]           Run a local daemon in the foreground with deployment
                          environment overrides
  status [options]        Observe the selected daemon and its published
                          endpoint
  stop [options]          Stop the selected daemon
  restart [options]       Restart the selected daemon worker, retaining its
                          supervisor launch
  reload [options]        Reload config.json without restarting
  pair [options]          Print the daemon pairing QR code and link
  config                  Read or edit persistent local configuration (local
                          operation)
  set-password [options]  Save a hashed daemon password (local operation)
  help [command]          display help for command

```

### paseo daemon start — Start the local daemon from persistent configuration
```
Usage: paseo daemon start [options]

Start the local daemon from persistent configuration (local operation)

Options:
  --home <path>        Local daemon home (default: ~/.paseo)
  --timeout <seconds>  Readiness deadline (default: 600)
  -h, --help           display help for command

```

### paseo daemon run — Run a local daemon in the foreground with deployment
```
Usage: paseo daemon run [options]

Run a local daemon in the foreground with deployment environment overrides

Options:
  --home <path>  Local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo daemon status — Observe the selected daemon and its published
```
Usage: paseo daemon status [options]

Observe the selected daemon and its published endpoint

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo daemon stop — Stop the selected daemon
```
Usage: paseo daemon stop [options]

Stop the selected daemon

Options:
  --json                    Output in JSON format
  --host <host>             Explicit daemon endpoint: host:port, TCP, socket,
                            SSH, or pairing URL (default: local home)
  --home <path>             Select a local daemon home (default: ~/.paseo)
  --timeout <seconds>       Graceful exit deadline (default: 15)
  --force                   Permit forced local process cleanup
  --keep-terminals          Leave terminals running; the next daemon started
                            for this home takes them back
  --kill-timeout <seconds>  Forced exit deadline (default: 3)
  -h, --help                display help for command

```

### paseo daemon restart — Restart the selected daemon worker, retaining its
```
Usage: paseo daemon restart [options]

Restart the selected daemon worker, retaining its supervisor launch

Options:
  --json               Output in JSON format
  --host <host>        Explicit daemon endpoint: host:port, TCP, socket, SSH,
                       or pairing URL (default: local home)
  --home <path>        Select a local daemon home (default: ~/.paseo)
  --timeout <seconds>  Replacement readiness deadline (default: 600)
  --kill-terminals     Close every terminal instead of keeping them across the
                       restart
  -h, --help           display help for command

```

### paseo daemon reload — Reload config.json without restarting
```
Usage: paseo daemon reload [options]

Reload config.json without restarting

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo daemon pair — Print the daemon pairing QR code and link
```
Usage: paseo daemon pair [options]

Print the daemon pairing QR code and link

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  --relay        Enable relay without prompting
  -h, --help     display help for command

```

### paseo daemon config — Read or edit persistent local configuration (local
```
Usage: paseo daemon config [options] [command]

Read or edit persistent local configuration (local operation)

Options:
  -h, --help                    display help for command

Commands:
  get [options] [path]
  set [options] <path> <value>
  unset [options] <path>
  help [command]                display help for command

```

### paseo daemon set-password — Save a hashed daemon password (local operation)
```
Usage: paseo daemon set-password [options]

Save a hashed daemon password (local operation)

Options:
  --home <path>  Local daemon home (default: ~/.paseo)
  --json         Output in JSON format
  -h, --help     display help for command

```

## paseo hub — Manage Paseo Hub
```
Usage: paseo hub [options] [command]

Manage Paseo Hub

Options:
  -o, --format <format>         output format: table, json, yaml (default:
                                "table")
  --json                        output in JSON format (alias for --format json)
  -q, --quiet                   minimal output (IDs only)
  --no-headers                  omit table headers
  --no-color                    disable colored output
  --host <host>                 Explicit daemon endpoint: host:port, TCP,
                                socket, SSH, or pairing URL (default: local
                                home)
  --home <path>                 Select a local daemon home (default: ~/.paseo)
  -v, --version                 output the version number
  -h, --help                    display help for command

Commands:
  login [options] [origin]      Log in to a Paseo Hub for CLI access
  init                          Create and optionally deploy a safe starter Hub
                                trigger
  connect [options] [origin]    Enroll this daemon with a Paseo Hub
  status [options]
  disconnect [options]
  permissions                   Manage this Hub's daemon permissions
  projects [options]            List projects for the authenticated Hub
                                organization
  export [options] [directory]  Export active Hub triggers as one YAML file per
                                trigger
  deploy [options]              Deploy .paseo organization triggers or a legacy
                                project bundle
  logout [options]              Remove the active stored Hub CLI login
  help [command]                display help for command

Hub origin precedence: command origin/--hub, PASEO_HUB_URL, active stored login, then https://hub.paseo.sh.
Credential precedence: --api-key, PASEO_HUB_API_KEY, then a stored login for the exact resolved origin.


```

### paseo hub init — Create and optionally deploy a safe starter Hub
```
Usage: paseo hub init [options]

Create and optionally deploy a safe starter Hub trigger

Options:
  -h, --help  display help for command

```

### paseo hub permissions — Manage this Hub's daemon permissions
```
Usage: paseo hub permissions [options] [command]

Manage this Hub's daemon permissions

Options:
  -h, --help                     display help for command

Commands:
  list [options]
  grant [options] <permission>
  revoke [options] <permission>
  help [command]                 display help for command

```

### paseo hub projects — List projects for the authenticated Hub
```
Usage: paseo hub projects [options]

List projects for the authenticated Hub organization

Options:
  --hub <origin>      Paseo Hub origin
  --api-key <secret>  Organization API key
  --json              Output in JSON format
  -h, --help          display help for command

Hub origin precedence: command origin/--hub, PASEO_HUB_URL, active stored login, then https://hub.paseo.sh.
Credential precedence: --api-key, PASEO_HUB_API_KEY, then a stored login for the exact resolved origin.


```

### paseo hub deploy — Deploy .paseo organization triggers or a legacy
```
Usage: paseo hub deploy [options]

Deploy .paseo organization triggers or a legacy project bundle

Options:
  -p, --project <slug>  Deploy a legacy bundle to this project slug
  --hub <origin>        Paseo Hub origin
  --api-key <secret>    Organization API key
  --dry-run             Validate without installing or activating
  --json                Output in JSON format
  -h, --help            display help for command

Hub origin precedence: command origin/--hub, PASEO_HUB_URL, active stored login, then https://hub.paseo.sh.
Credential precedence: --api-key, PASEO_HUB_API_KEY, then a stored login for the exact resolved origin.


```

### paseo hub logout — Remove the active stored Hub CLI login
```
Usage: paseo hub logout [options]

Remove the active stored Hub CLI login

Options:
  --disconnect-daemon  Also disconnect a daemon related to the same Hub
  --force              Remove daemon authority without notifying the Hub
  --json               Output in JSON format
  --host <host>        Explicit daemon endpoint: host:port, TCP, socket, SSH,
                       or pairing URL (default: local home)
  --home <path>        Select a local daemon home (default: ~/.paseo)
  -h, --help           display help for command

```

## paseo terminal — Manage workspace terminals
```
Usage: paseo terminal [options] [command]

Manage workspace terminals

Options:
  -o, --format <format>                        output format: table, json, yaml (default: "table")
  --json                                       output in JSON format (alias for --format json)
  -q, --quiet                                  minimal output (IDs only)
  --no-headers                                 omit table headers
  --no-color                                   disable colored output
  --host <host>                                Explicit daemon endpoint: host:port, TCP, socket, SSH, or pairing URL (default: local home)
  --home <path>                                Select a local daemon home (default: ~/.paseo)
  -v, --version                                output the version number
  -h, --help                                   display help for command

Commands:
  ls [options]                                 List terminals
  create [options]                             Create a terminal
  kill [options] <terminal-id>                 Kill a terminal
  capture [options] <terminal-id>              Capture terminal output
  send-keys [options] <terminal-id> <keys...>  Send keys to a terminal
  help [command]                               display help for command

```

### paseo terminal ls — List terminals
```
Usage: paseo terminal ls [options]

List terminals

Options:
  --all             List terminals across all workspaces
  --workspace <id>  Workspace ID
  --cwd <path>      Working directory
  --json            Output in JSON format
  --host <host>     Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                    pairing URL (default: local home)
  --home <path>     Select a local daemon home (default: ~/.paseo)
  -h, --help        display help for command

```

### paseo terminal create — Create a terminal
```
Usage: paseo terminal create [options]

Create a terminal

Options:
  --workspace <id>  Workspace ID
  --cwd <path>      Working directory
  --name <name>     Terminal name
  --json            Output in JSON format
  --host <host>     Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                    pairing URL (default: local home)
  --home <path>     Select a local daemon home (default: ~/.paseo)
  -h, --help        display help for command

```

### paseo terminal kill — Kill a terminal
```
Usage: paseo terminal kill [options] <terminal-id>

Kill a terminal

Arguments:
  terminal-id    Terminal ID, ID prefix, or name

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo terminal capture — Capture terminal output
```
Usage: paseo terminal capture [options] <terminal-id>

Capture terminal output

Arguments:
  terminal-id       Terminal ID, ID prefix, or name

Options:
  --start <n>       Capture start line
  --end <n>         Capture end line
  -S, --scrollback  Capture from the beginning of scrollback
  --ansi            Preserve ANSI escape codes
  --json            Output in JSON format
  --host <host>     Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                    pairing URL (default: local home)
  --home <path>     Select a local daemon home (default: ~/.paseo)
  -h, --help        display help for command

```

### paseo terminal send-keys — Send keys to a terminal
```
Usage: paseo terminal send-keys [options] <terminal-id> <keys...>

Send keys to a terminal

Arguments:
  terminal-id    Terminal ID, ID prefix, or name
  keys           Keys to send

Options:
  -l, --literal  Send raw keys without interpreting special tokens
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

## paseo script — Manage configured workspace scripts
```
Usage: paseo script [options] [command]

Manage configured workspace scripts

Options:
  -o, --format <format>   output format: table, json, yaml (default: "table")
  --json                  output in JSON format (alias for --format json)
  -q, --quiet             minimal output (IDs only)
  --no-headers            omit table headers
  --no-color              disable colored output
  --host <host>           Explicit daemon endpoint: host:port, TCP, socket,
                          SSH, or pairing URL (default: local home)
  --home <path>           Select a local daemon home (default: ~/.paseo)
  -v, --version           output the version number
  -h, --help              display help for command

Commands:
  ls [options]            List configured workspace scripts
  start [options] <name>  Start a configured workspace script
  stop [options] <name>   Stop a running workspace script
  help [command]          display help for command

```

### paseo script ls — List configured workspace scripts
```
Usage: paseo script ls [options]

List configured workspace scripts

Options:
  --cwd <path>                Workspace directory (default: current directory)
  --workspace <workspace-id>  Workspace ID (required when a directory has
                              multiple workspaces)
  --json                      Output in JSON format
  --host <host>               Explicit daemon endpoint: host:port, TCP, socket,
                              SSH, or pairing URL (default: local home)
  --home <path>               Select a local daemon home (default: ~/.paseo)
  -h, --help                  display help for command

```

### paseo script start — Start a configured workspace script
```
Usage: paseo script start [options] <name>

Start a configured workspace script

Options:
  --cwd <path>                Workspace directory (default: current directory)
  --workspace <workspace-id>  Workspace ID (required when a directory has
                              multiple workspaces)
  --json                      Output in JSON format
  --host <host>               Explicit daemon endpoint: host:port, TCP, socket,
                              SSH, or pairing URL (default: local home)
  --home <path>               Select a local daemon home (default: ~/.paseo)
  -h, --help                  display help for command

```

### paseo script stop — Stop a running workspace script
```
Usage: paseo script stop [options] <name>

Stop a running workspace script

Options:
  --cwd <path>                Workspace directory (default: current directory)
  --workspace <workspace-id>  Workspace ID (required when a directory has
                              multiple workspaces)
  --json                      Output in JSON format
  --host <host>               Explicit daemon endpoint: host:port, TCP, socket,
                              SSH, or pairing URL (default: local home)
  --home <path>               Select a local daemon home (default: ~/.paseo)
  -h, --help                  display help for command

```

## paseo schedule — Manage recurring schedules
```
Usage: paseo schedule [options] [command]

Manage recurring schedules

Options:
  -o, --format <format>      output format: table, json, yaml (default:
                             "table")
  --json                     output in JSON format (alias for --format json)
  -q, --quiet                minimal output (IDs only)
  --no-headers               omit table headers
  --no-color                 disable colored output
  --host <host>              Explicit daemon endpoint: host:port, TCP, socket,
                             SSH, or pairing URL (default: local home)
  --home <path>              Select a local daemon home (default: ~/.paseo)
  -v, --version              output the version number
  -h, --help                 display help for command

Commands:
  create [options] <prompt>  Create a schedule
  ls [options]               List schedules
  inspect [options] <id>     Inspect a schedule
  logs [options] <id>        Show recent schedule run logs
  pause [options] <id>       Pause a schedule
  resume [options] <id>      Resume a paused schedule
  delete [options] <id>      Delete a schedule
  run-once [options] <id>    Manually trigger a single run of a schedule
                             without affecting cadence
  update [options] <id>      Update an existing schedule in place
  help [command]             display help for command

```

### paseo schedule create — Create a schedule
```
Usage: paseo schedule create [options] <prompt>

Create a schedule

Arguments:
  prompt                   Prompt to run on the schedule

Options:
  --every <duration>       Cron-compatible cadence preset (for example: 5m, 1h)
  --cron <expr>            Cron cadence expression
  --timezone <iana>        IANA time zone for cron cadence (default: UTC)
  --name <name>            Optional schedule name
  --provider <provider>    Agent provider, or provider/model (e.g. codex or
                           codex/gpt-5.4)
  --mode <mode>            Provider-specific mode (e.g. claude
                           bypassPermissions, opencode build)
  --thinking <id>          Thinking option ID for new-agent runs
  --cwd <path>             Working directory (default: current; required with
                           --host)
  --run-now                Fire one immediate run on creation
  --max-runs <n>           Maximum number of runs
  --expires-in <duration>  Time to live for the schedule
  --json                   Output in JSON format
  --host <host>            Explicit daemon endpoint: host:port, TCP, socket,
                           SSH, or pairing URL (default: local home)
  --home <path>            Select a local daemon home (default: ~/.paseo)
  -h, --help               display help for command

```

### paseo schedule ls — List schedules
```
Usage: paseo schedule ls [options]

List schedules

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo schedule inspect — Inspect a schedule
```
Usage: paseo schedule inspect [options] <id>

Inspect a schedule

Arguments:
  id             Schedule ID

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo schedule logs — Show recent schedule run logs
```
Usage: paseo schedule logs [options] <id>

Show recent schedule run logs

Arguments:
  id             Schedule ID

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo schedule pause — Pause a schedule
```
Usage: paseo schedule pause [options] <id>

Pause a schedule

Arguments:
  id             Schedule ID

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo schedule resume — Resume a paused schedule
```
Usage: paseo schedule resume [options] <id>

Resume a paused schedule

Arguments:
  id             Schedule ID

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo schedule delete — Delete a schedule
```
Usage: paseo schedule delete [options] <id>

Delete a schedule

Arguments:
  id             Schedule ID

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo schedule run-once — Manually trigger a single run of a schedule
```
Usage: paseo schedule run-once [options] <id>

Manually trigger a single run of a schedule without affecting cadence

Arguments:
  id             Schedule ID

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo schedule update — Update an existing schedule in place
```
Usage: paseo schedule update [options] <id>

Update an existing schedule in place

Arguments:
  id                       Schedule ID

Options:
  --every <duration>       Cron-compatible cadence preset (for example: 5m, 1h)
  --cron <expr>            Switch to cron cadence expression
  --timezone <iana>        IANA time zone for cron cadence (requires --cron)
  --name <name>            Rename the schedule (empty string clears the name)
  --prompt <text>          Replace the schedule prompt
  --provider <provider>    New agent provider, or provider/model (only for
                           new-agent target)
  --model <model>          New agent model (only for new-agent target)
  --mode <mode>            New agent provider mode (only for new-agent target)
  --cwd <path>             New working directory (only for new-agent target)
  --max-runs <n>           Set or change maximum number of runs
  --no-max-runs            Clear the max-runs limit
  --expires-in <duration>  Set or change time to live for the schedule
  --no-expires-in          Clear the expiration
  --json                   Output in JSON format
  --host <host>            Explicit daemon endpoint: host:port, TCP, socket,
                           SSH, or pairing URL (default: local home)
  --home <path>            Select a local daemon home (default: ~/.paseo)
  -h, --help               display help for command

```

## paseo heartbeat — Manage this agent's heartbeats
```
Usage: paseo heartbeat [options] [command]

Manage this agent's heartbeats

Options:
  -o, --format <format>      output format: table, json, yaml (default:
                             "table")
  --json                     output in JSON format (alias for --format json)
  -q, --quiet                minimal output (IDs only)
  --no-headers               omit table headers
  --no-color                 disable colored output
  --host <host>              Explicit daemon endpoint: host:port, TCP, socket,
                             SSH, or pairing URL (default: local home)
  --home <path>              Select a local daemon home (default: ~/.paseo)
  -v, --version              output the version number
  -h, --help                 display help for command

Commands:
  create [options] <prompt>  Create a recurring prompt for this agent
  update [options] <id>      Change a heartbeat cron cadence
  delete [options] <id>      Delete a heartbeat
  help [command]             display help for command

```

### paseo heartbeat create — Create a recurring prompt for this agent
```
Usage: paseo heartbeat create [options] <prompt>

Create a recurring prompt for this agent

Arguments:
  prompt                   Prompt to send

Options:
  --cron <expr>            Five-field cron cadence
  --timezone <iana>        IANA time zone
  --name <name>            Heartbeat name
  --max-runs <n>           Maximum number of runs
  --expires-in <duration>  Time to live
  --json                   Output in JSON format
  --host <host>            Explicit daemon endpoint: host:port, TCP, socket,
                           SSH, or pairing URL (default: local home)
  --home <path>            Select a local daemon home (default: ~/.paseo)
  -h, --help               display help for command

```

### paseo heartbeat update — Change a heartbeat cron cadence
```
Usage: paseo heartbeat update [options] <id>

Change a heartbeat cron cadence

Arguments:
  id                 Heartbeat ID

Options:
  --cron <expr>      Five-field cron cadence
  --timezone <iana>  IANA time zone
  --json             Output in JSON format
  --host <host>      Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                     pairing URL (default: local home)
  --home <path>      Select a local daemon home (default: ~/.paseo)
  -h, --help         display help for command

```

### paseo heartbeat delete — Delete a heartbeat
```
Usage: paseo heartbeat delete [options] <id>

Delete a heartbeat

Arguments:
  id             Heartbeat ID

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

## paseo permit — Manage permission requests
```
Usage: paseo permit [options] [command]

Manage permission requests

Options:
  -o, --format <format>             output format: table, json, yaml (default:
                                    "table")
  --json                            output in JSON format (alias for --format
                                    json)
  -q, --quiet                       minimal output (IDs only)
  --no-headers                      omit table headers
  --no-color                        disable colored output
  --host <host>                     Explicit daemon endpoint: host:port, TCP,
                                    socket, SSH, or pairing URL (default: local
                                    home)
  --home <path>                     Select a local daemon home (default:
                                    ~/.paseo)
  -v, --version                     output the version number
  -h, --help                        display help for command

Commands:
  ls [options]                      List all pending permissions
  allow [options] <agent> [req_id]  Allow a permission request
  deny [options] <agent> [req_id]   Deny a permission request
  help [command]                    display help for command

```

### paseo permit ls — List all pending permissions
```
Usage: paseo permit ls [options]

List all pending permissions

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

## paseo provider — Manage agent providers
```
Usage: paseo provider [options] [command]

Manage agent providers

Options:
  -o, --format <format>            output format: table, json, yaml (default:
                                   "table")
  --json                           output in JSON format (alias for --format
                                   json)
  -q, --quiet                      minimal output (IDs only)
  --no-headers                     omit table headers
  --no-color                       disable colored output
  --host <host>                    Explicit daemon endpoint: host:port, TCP,
                                   socket, SSH, or pairing URL (default: local
                                   home)
  --home <path>                    Select a local daemon home (default:
                                   ~/.paseo)
  -v, --version                    output the version number
  -h, --help                       display help for command

Commands:
  ls [options]                     List available providers and status
  models [options] <provider>      List models for a provider
  diagnostic [options] <provider>  Show provider installation, environment, and
                                   availability diagnostics
  help [command]                   display help for command

```

### paseo provider ls — List available providers and status
```
Usage: paseo provider ls [options]

List available providers and status

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo provider models — List models for a provider
```
Usage: paseo provider models [options] <provider>

List models for a provider

Arguments:
  provider       Provider name (claude, codex, opencode)

Options:
  --thinking     Include thinking option IDs for each model
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo provider diagnostic — Show provider installation, environment, and
```
Usage: paseo provider diagnostic [options] <provider>

Show provider installation, environment, and availability diagnostics

Arguments:
  provider       Provider name

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

## paseo usage — Show account usage reports
```
Usage: paseo usage [options] [command]

Show account usage reports

Options:
  --json                         Output in JSON format
  --host <host>                  Explicit daemon endpoint: host:port, TCP,
                                 socket, SSH, or pairing URL (default: local
                                 home)
  --home <path>                  Select a local daemon home (default: ~/.paseo)
  --agent <id>                   Show account quota associated with this agent
                                 (not agent token consumption)
  --refresh                      Refresh usage instead of using the host cache
  -o, --format <format>          output format: table, json, yaml (default:
                                 "table")
  -q, --quiet                    minimal output (IDs only)
  --no-headers                   omit table headers
  --no-color                     disable colored output
  -v, --version                  output the version number
  -h, --help                     display help for command

Commands:
  ls [options]                   List account quotas and balances
  inspect [options] <report-id>  Show a complete account usage report

```

### paseo usage ls — List account quotas and balances
```
Usage: paseo usage ls [options]

List account quotas and balances

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  --agent <id>   Show account quota associated with this agent (not agent token
                 consumption)
  --refresh      Refresh usage instead of using the host cache
  -h, --help     display help for command

```

### paseo usage inspect — Show a complete account usage report
```
Usage: paseo usage inspect [options] <report-id>

Show a complete account usage report

Arguments:
  report-id      Exact opaque Report ID

Options:
  --refresh      Refresh the selected report instead of using the host cache
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

## paseo plugin — Manage trusted, unsandboxed plugins
```
Usage: paseo plugin [options] [command]

Manage trusted, unsandboxed plugins

Options:
  -o, --format <format>           output format: table, json, yaml (default:
                                  "table")
  --json                          output in JSON format (alias for --format
                                  json)
  -q, --quiet                     minimal output (IDs only)
  --no-headers                    omit table headers
  --no-color                      disable colored output
  --host <host>                   Explicit daemon endpoint: host:port, TCP,
                                  socket, SSH, or pairing URL (default: local
                                  home)
  --home <path>                   Select a local daemon home (default:
                                  ~/.paseo)
  -h, --help                      display help for command

Commands:
  init [options] <directory>      Create a typecheckable local plugin
  ls [options] [id]               List configured plugins
  logs [options] <id>             Show recent plugin output
  install|add [options] <source>  Trust and install a plugin from a registry,
                                  directory, Git repository, or npm package
  update [options] [id]           Review and update installed plugins
  reload [options] <id>           reload a plugin
  enable [options] <id>           enable a plugin
  disable [options] <id>          disable a plugin
  remove [options] <id>           Remove plugin configuration
  help [command]                  display help for command

```

### paseo plugin init — Create a typecheckable local plugin
```
Usage: paseo plugin init [options] <directory>

Create a typecheckable local plugin

Options:
  --id <id>   Manifest plugin ID (defaults to the directory name)
  --json      Output in JSON format
  -h, --help  display help for command

```

### paseo plugin logs — Show recent plugin output
```
Usage: paseo plugin logs [options] <id>

Show recent plugin output

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo plugin reload — reload a plugin
```
Usage: paseo plugin reload [options] <id>

reload a plugin

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo plugin enable — enable a plugin
```
Usage: paseo plugin enable [options] <id>

enable a plugin

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo plugin disable — disable a plugin
```
Usage: paseo plugin disable [options] <id>

disable a plugin

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo plugin remove — Remove plugin configuration
```
Usage: paseo plugin remove [options] <id>

Remove plugin configuration

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

## paseo speech — Speech commands
```
Usage: paseo speech [options]

Speech commands

Options:
  -o, --format <format>  output format: table, json, yaml (default: "table")
  --json                 output in JSON format (alias for --format json)
  -q, --quiet            minimal output (IDs only)
  --no-headers           omit table headers
  --no-color             disable colored output
  --host <host>          Explicit daemon endpoint: host:port, TCP, socket, SSH,
                         or pairing URL (default: local home)
  --home <path>          Select a local daemon home (default: ~/.paseo)
  -v, --version          output the version number
  -h, --help             display help for command

```

## paseo project — Manage projects
```
Usage: paseo project [options] [command]

Manage projects

Options:
  -o, --format <format>                 output format: table, json, yaml
                                        (default: "table")
  --json                                output in JSON format (alias for
                                        --format json)
  -q, --quiet                           minimal output (IDs only)
  --no-headers                          omit table headers
  --no-color                            disable colored output
  --host <host>                         Explicit daemon endpoint: host:port,
                                        TCP, socket, SSH, or pairing URL
                                        (default: local home)
  --home <path>                         Select a local daemon home (default:
                                        ~/.paseo)
  -v, --version                         output the version number
  -h, --help                            display help for command

Commands:
  create [options] [path]               Register a project directory
  ls [options]                          List projects
  rename [options] <project-id> [name]  Set a project's user-visible name
  delete [options] <project-id>         Delete a project and its workspaces
  help [command]                        display help for command

```

### paseo project ls — List projects
```
Usage: paseo project ls [options]

List projects

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo project delete — Delete a project and its workspaces
```
Usage: paseo project delete [options] <project-id>

Delete a project and its workspaces

Arguments:
  project-id     Project id

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

## paseo workspace — Manage workspaces
```
Usage: paseo workspace [options] [command]

Manage workspaces

Options:
  -o, --format <format>                    output format: table, json, yaml (default: "table")
  --json                                   output in JSON format (alias for --format json)
  -q, --quiet                              minimal output (IDs only)
  --no-headers                             omit table headers
  --no-color                               disable colored output
  --host <host>                            Explicit daemon endpoint: host:port, TCP, socket, SSH, or pairing URL (default: local home)
  --home <path>                            Select a local daemon home (default: ~/.paseo)
  -v, --version                            output the version number
  -h, --help                               display help for command

Commands:
  setup [options] <workspace-id>           Allow and run setup for a workspace
  create [options]                         Create a workspace
  ls [options]                             List active workspaces
  rename [options] <workspace-id> [title]  Set a workspace's user-visible title
  archive [options] <workspace-id>         Archive a workspace and everything it owns
  help [command]                           display help for command

```

### paseo workspace setup — Allow and run setup for a workspace
```
Usage: paseo workspace setup [options] <workspace-id>

Allow and run setup for a workspace

Arguments:
  workspace-id   Workspace id

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo workspace create — Create a workspace
```
Usage: paseo workspace create [options]

Create a workspace

Options:
  --isolation <local|worktree>  Workspace isolation
  --path <path>                 Local directory or source checkout (default:
                                current)
  --project <id>                Existing project id
  --title <title>               Workspace title
  --mode <mode>                 Worktree mode: branch-off, checkout-branch, or
                                checkout-pr (default: branch-off)
  --worktree-slug <slug>        Managed worktree path slug
  --new-branch <name>           New branch name (--mode branch-off)
  --base <ref>                  Base ref (--mode branch-off)
  --branch <name>               Existing branch (--mode checkout-branch)
  --pr-number <n>               Pull request or change request number (--mode
                                checkout-pr)
  --forge <forge>               Forge for --mode checkout-pr (default: source
                                checkout)
  --background                  Create a background workspace
  --no-background               Create a visible workspace instead of
                                inheriting background
  --json                        Output in JSON format
  --host <host>                 Explicit daemon endpoint: host:port, TCP,
                                socket, SSH, or pairing URL (default: local
                                home)
  --home <path>                 Select a local daemon home (default: ~/.paseo)
  -h, --help                    display help for command

```

### paseo workspace ls — List active workspaces
```
Usage: paseo workspace ls [options]

List active workspaces

Options:
  --background   Include background workspaces
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```

### paseo workspace archive — Archive a workspace and everything it owns
```
Usage: paseo workspace archive [options] <workspace-id>

Archive a workspace and everything it owns

Arguments:
  workspace-id   Workspace id

Options:
  --json         Output in JSON format
  --host <host>  Explicit daemon endpoint: host:port, TCP, socket, SSH, or
                 pairing URL (default: local home)
  --home <path>  Select a local daemon home (default: ~/.paseo)
  -h, --help     display help for command

```
