---
name: install-devstack
description: Install devstack CLI and daemon on a machine (Linux or macOS). Use when setting up devstack from scratch or upgrading.
metadata:
  short-description: Install devstack CLI and daemon
---

# Install Devstack

This is an interactive, agent-led installation. Walk the user through each step, confirming choices before proceeding.

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

## Done

Confirm to the user that devstack is installed and ready. Suggest next steps:
- `cd <project> && devstack init` to create a config
- Load the `devstack` skill for help setting up services

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

The install script automatically restarts the daemon after building.

---

## Troubleshooting

### Daemon won't start

Run in foreground to see errors:

```bash
devstack daemon
```

### macOS PATH issues

LaunchAgents inherit a minimal PATH. If services need tools like `pnpm` or `poetry`:
- Use absolute paths in `devstack.toml` commands (e.g. `/opt/homebrew/bin/pnpm dev`)
- Or set PATH in your service's `env` config

### Linux: "systemd user instance unavailable"

Enable lingering so user services run without an active login session:

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
rm -rf ~/.local/share/devstack        # Linux data
rm -rf ~/Library/Application\ Support/devstack  # macOS data
```
