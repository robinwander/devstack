---
name: install-devstack
description: Install devstack CLI and daemon on a machine (Linux or macOS). Use when setting up devstack from scratch or upgrading.
metadata:
  short-description: Install devstack CLI and daemon
---

# Install Devstack

This is an interactive, agent-led installation. Walk the user through each step, confirming choices before proceeding.

## Prerequisites

- **Rust toolchain** — needed for building from source. Install via [rustup](https://rustup.rs/) if missing.
- **Node.js + pnpm (or npm)** — required for the web dashboard. The install script runs `pnpm install` (or falls back to `npm ci`) to set up dashboard dependencies.
- **Git** — to clone the repo.
- **Linux**: systemd with user session support (most distros).
- **macOS**: No extra requirements. The daemon runs as a LaunchAgent.

## Step 1: Check if already installed

Run silently — don't ask the user, just check:

```bash
command -v devstack && devstack doctor
```

- **Both succeed** → Tell the user devstack is already installed and healthy. Ask if they want to **upgrade** instead (skip to Upgrading section).
- **`command -v` succeeds but `doctor` fails** → The CLI exists but the daemon is broken. Skip to Step 4 (daemon install) after confirming with the user.
- **Neither succeeds** → Fresh install. Continue to Step 2.

## Step 2: Detect platform

Run silently:

```bash
uname -s  # Darwin or Linux
uname -m  # x86_64 or arm64/aarch64
```

Tell the user what you detected (e.g. "You're on macOS ARM64"). This determines the install path.

**Linux only**: Verify systemd user session is available:

```bash
systemctl --user status
```

If this fails, warn the user that devstack requires systemd with user session support. Suggest `loginctl enable-linger $(whoami)` if relevant, but don't block — they may want to proceed anyway.

## Step 3: Choose install method

Ask the user:

> **How would you like to install devstack?**
>
> 1. **Download prebuilt binary** (recommended, fastest — no build tools needed)
> 2. **Build from source** (requires Rust toolchain)

### Option 1: Prebuilt binary

Run the installer script:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/robinwander/devstack/releases/latest/download/devstack-installer.sh | sh
```

This downloads the correct binary for the platform and installs to `~/.local/bin/devstack`.

If `~/.local/bin` is not on PATH, tell the user and offer to add it:

```bash
# bash/zsh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc

# fish
fish_add_path ~/.local/bin
```

### Option 2: Build from source

Check for Rust toolchain:

```bash
command -v cargo
```

If missing, ask the user if you should install it:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then clone and build:

```bash
git clone https://github.com/robinwander/devstack.git ~/tools/devstack
cd ~/tools/devstack
./scripts/install-cli.sh
```

This builds the CLI binary to `~/.local/bin/devstack` and installs the web dashboard to `~/.local/share/devstack/dashboard` (Linux) or `~/Library/Application Support/devstack/dashboard` (macOS).

If `~/.local/bin` is not on PATH, same guidance as Option 1.

## Step 4: Install and start the daemon

Run:

```bash
devstack install
```

Explain what this does:
- **Linux**: Creates and enables a systemd user service. Daemon starts immediately and auto-starts on login.
- **macOS**: Creates a LaunchAgent plist. Daemon starts immediately and auto-starts on login.

## Step 5: Verify

Run:

```bash
devstack doctor
```

All checks should report `ok`. Expected checks:
- `daemon_socket` — daemon is running and reachable
- `systemd_user` (Linux) / `process_manager` (macOS) — process manager works
- `filesystem` — data directories are writable

If any check fails, see Troubleshooting below before continuing.

## Step 6: Shell completions

Ask the user:

> **Want to install shell completions?** They give you tab-completion for all devstack commands and flags.

Detect their shell from `$SHELL` and run the appropriate command:

```bash
# bash
mkdir -p ~/.local/share/bash-completion/completions
devstack completions bash > ~/.local/share/bash-completion/completions/devstack

# zsh
mkdir -p ~/.zsh/completions
devstack completions zsh > ~/.zsh/completions/_devstack
# Remind user: ensure ~/.zsh/completions is in fpath

# fish
devstack completions fish > ~/.config/fish/completions/devstack.fish
```

## Step 7 (optional): initialize a project

Confirm to the user that devstack is installed and healthy. Ask if they want to set up a project now.

### 7a. Identify the project

Ask the user which project to set up. If they give a name instead of a path, look for it relative to cwd or common locations (`~/`, `~/projects/`, `~/code/`, `~/repos/`). Then `cd` into the project root.

### 7b. Read project context

Before writing any config, understand the project. Read these files (whichever exist):

```bash
# Project docs — understand what the project does and how it runs
cat README.md AGENTS.md CLAUDE.md .github/CONTRIBUTING.md

# Package manifests — identify apps, scripts, and dependencies
cat package.json                  # Node.js: look at "scripts" for dev/start commands
cat apps/*/package.json           # Monorepo: check each app/package
cat pyproject.toml                # Python: look for scripts, entry points
cat Cargo.toml                    # Rust: binary targets
cat Makefile                      # Make targets (dev, run, serve, etc.)
cat docker-compose.yml            # Existing service definitions — good mapping source
cat Procfile                      # Heroku-style process definitions

# Existing env and config
cat .env .env.example .env.local  # Default env vars, required config
cat prisma/schema.prisma          # Database schema → need a db service + migrations
```

From this context, identify:
- **Services**: each long-running process the project needs (web servers, API servers, workers, databases, caches, message brokers)
- **Dependencies between services**: which services need to talk to each other (e.g. web → api → db)
- **Infrastructure**: databases, Redis, etc. that should be globals
- **Tasks**: one-shot commands (migrations, codegen, seed scripts, linting, builds)
- **Init tasks**: tasks that must run before a service starts (e.g. migrations before the API)

### 7c. Generate the config

```bash
devstack init   # creates a starter devstack.toml
```

Then edit `devstack.toml` based on what you discovered. Load the `devstack` skill for full config reference. Key decisions:

**Services**: For each app, create a service entry with:
- `cmd` — the dev-mode start command (e.g. `pnpm dev`, `cargo run`, `python manage.py runserver`)
- `deps` — services this one depends on (e.g. api depends on db)
- `watch` — source file patterns for change detection (e.g. `["src/**", "package.json"]`)
- `auto_restart = true` — if you want live file watching to auto-restart the service
- `readiness` — how to know the service is healthy:
  - Web servers / APIs → `readiness = { http = { path = "/health" } }` or `{ tcp = {} }`
  - Workers with no port → `readiness = { log_regex = "ready" }` or `{ delay_ms = 2000 }`
  - One-shot setup → `readiness = { exit = {} }` with `port = "none"`

**Environment wiring**: Services reference each other via templates:
```toml
[stacks.dev.services.web.env]
VITE_API_URL = "{{ services.api.url }}"
DATABASE_URL = "postgres://localhost:{{ services.db.port }}/myapp"
```

**Globals**: Infrastructure shared across stacks (databases, caches):
```toml
[globals.db]
cmd = "docker run --rm -p $PORT:5432 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:16"
readiness = { tcp = {} }

[globals.redis]
cmd = "redis-server --port $PORT"
readiness = { tcp = {} }
```

**Tasks**: Migrations, codegen, builds:
```toml
[tasks.migrate]
cmd = "prisma migrate dev"
watch = ["prisma/schema.prisma"]

[stacks.dev.services.api]
cmd = "pnpm dev"
init = ["migrate"]   # runs migrate before api starts
```

### 7d. Validate

```bash
devstack lint
```

Fix any errors. Then ask the user to review — are there services to add or remove? Apply their feedback and re-lint.

### 7e. Start the stack

Ask if they want to start:

```bash
devstack up
```

If services fail, debug systematically:

```bash
devstack status                          # which services are unhealthy?
devstack diagnose                        # port binding, systemd state, recent errors
devstack logs --service <name> --last 50 # check the failing service's output
```

Common issues to fix:
- **Port conflicts**: service binds to a hardcoded port → remove the hardcoded port and use `$PORT` (devstack allocates ports automatically)
- **Missing env vars**: service needs config that isn't wired up → add to `[stacks.dev.services.<name>.env]`
- **Wrong readiness probe**: service is running but devstack thinks it's not ready → adjust the `readiness` config (check what the service actually exposes)
- **Dependency ordering**: service starts before its dependency is ready → add to `deps`
- **macOS PATH**: command not found → use absolute path in `cmd` or set PATH in env

Iterate until `devstack status` shows all services healthy.

### 7f. Show the user around

Once the stack is running:

```bash
devstack ui                              # open the dashboard
devstack show --service api              # navigate dashboard to a specific service
```

Explain what they now have:
- `devstack up` to start, `devstack down` to stop
- `devstack status` to check health at a glance
- `devstack logs --service <name>` to query logs (with full-text search via `--search`)
- `devstack ui` for the web dashboard with real-time logs, facet filtering, and service management
- `devstack agent -- <cmd>` to wrap their AI agent with two-way dashboard integration

---

## Upgrading

### From prebuilt binary

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/robinwander/devstack/releases/latest/download/devstack-installer.sh | sh
devstack install
```

### From source

```bash
cd ~/tools/devstack
git pull
./scripts/install-cli.sh
```

The install script automatically restarts the daemon after upgrading.

## Troubleshooting

### Daemon won't start

Run in foreground to see errors:

```bash
devstack daemon
```

### macOS PATH issues

LaunchAgents inherit a minimal PATH. If services need tools like `pnpm` or `poetry`, either:

- Use absolute paths in `devstack.toml` commands (e.g. `/opt/homebrew/bin/pnpm dev`)
- Or add a PATH override to the LaunchAgent plist

### Linux: "systemd user instance unavailable"

Ensure lingering is enabled so user services run without an active login:

```bash
loginctl enable-linger $(whoami)
```

### Uninstall

```bash
# Linux
systemctl --user stop devstack.service
systemctl --user disable devstack.service
rm ~/.config/systemd/user/devstack.service
systemctl --user daemon-reload

# macOS
launchctl unload ~/Library/LaunchAgents/devstack.plist
rm ~/Library/LaunchAgents/devstack.plist

# Both
rm ~/.local/bin/devstack
rm -rf ~/.local/share/devstack  # Linux data
rm -rf ~/Library/Application\ Support/devstack  # macOS data
```
