#!/usr/bin/env python3
import os
import signal
import socketserver
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

port = int(os.environ[os.environ.get("FIXTURE_PORT_ENV", "PORT")])
service_name = os.environ.get("FIXTURE_SERVICE_NAME", "service")
response_body = os.environ.get("FIXTURE_RESPONSE_BODY", "ok")
start_marker = os.environ.get("FIXTURE_STARTS_FILE")
ready_marker = os.environ.get("FIXTURE_READY_FILE")

if start_marker:
    path = Path(start_marker)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write("started\n")

if ready_marker:
    path = Path(ready_marker)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write("ready\n")

print(f"service-started name={service_name} port={port}", flush=True)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = response_body.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        message = fmt % args
        print(f"http-access name={service_name} {message}", flush=True)


class ReusableHTTPServer(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


server = ReusableHTTPServer(("127.0.0.1", port), Handler)


def shutdown(_signum, _frame):
    print(f"service-stopping name={service_name}", flush=True)
    import threading

    threading.Thread(target=server.shutdown, daemon=True).start()


signal.signal(signal.SIGTERM, shutdown)
signal.signal(signal.SIGINT, shutdown)

try:
    server.serve_forever()
finally:
    server.server_close()
    print(f"service-stopped name={service_name}", flush=True)
    sys.stdout.flush()
