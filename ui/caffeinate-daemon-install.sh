#!/usr/bin/env bash
set -euo pipefail

PLIST_PATH="/Library/LaunchDaemons/com.user.caffeinate.plist"

cat << "PLIST" > "$PLIST_PATH"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.user.caffeinate</string>

    <key>ProgramArguments</key>
    <array>
      <string>/usr/bin/caffeinate</string>
      <string>-d</string>
      <string>-i</string>
    </array>

    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
  </dict>
</plist>
PLIST

chown root:wheel "$PLIST_PATH"
chmod 644 "$PLIST_PATH"

launchctl bootstrap system "$PLIST_PATH" || true
launchctl enable system/com.user.caffeinate
launchctl kickstart -k system/com.user.caffeinate

pgrep -af caffeinate || true
launchctl print system/com.user.caffeinate | rg -n "state|pid|last exit status" || true
