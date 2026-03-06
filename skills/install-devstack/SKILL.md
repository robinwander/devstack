---
name: install-devstack
description: Install devstack CLI and daemon on a machine (Linux or macOS). Use when setting up devstack from scratch or upgrading.
metadata:
  short-description: Install devstack from source
---

# Install Devstack

## Prerequisites

- **Rust toolchain** — `cargo` must be on PATH. If missing, install via [rustup](https://rustup.rs/):
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **Git** — to clone the repo.
- **Linux**: systemd with user session support (most distros). Verify: `systemctl --user status`.
- **macOS**: No extra requirements. The daemon runs as a LaunchAgent.

## Pre-check

Before installing, check if devstack is already available:
```bash
command -v devstack && devstack doctor
```
If both succeed, devstack is already installed. Use the **Upgrading** section instead.

## Steps

### 1. Clone the repo

```bash
git clone https://github.com/robinwander/devstack.git ~/tools/devstack
cd ~/tools/devstack
```

### 2. Build and install the CLI

```bash
./scripts/install-cli.sh
```

This builds a release binary and installs it to `~/.local/bin/devstack`. If `~/.local/bin` is not in your PATH, add it:

```bash
# bash/zsh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
# fish
fish_add_path ~/.local/bin
```

Verify: `devstack --help`

### 3. Install the daemon

```bash
devstack install
```

- **Linux**: Creates and enables a systemd user service (`devstack.service`). The daemon starts immediately and auto-starts on login.
- **macOS**: Creates a LaunchAgent (`~/Library/LaunchAgents/devstack.plist`). The daemon starts immediately and auto-starts on login.

### 4. Verify

```bash
devstack doctor
```

All checks should report `ok`. Expected checks:
- `daemon_socket` — daemon is running and socket is reachable
- `systemd_user` (Linux) / `process_manager` (macOS) — process manager works
- `filesystem` — base directories are writable

### 5. Install shell completions (optional)

```bash
# bash
devstack completions bash > ~/.local/share/bash-completion/completions/devstack

# zsh
mkdir -p ~/.zsh/completions
devstack completions zsh > ~/.zsh/completions/_devstack

# fish
devstack completions fish > ~/.config/fish/completions/devstack.fish
```

## Upgrading

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
