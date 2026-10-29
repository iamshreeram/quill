#!/usr/bin/env python3
# Local fixture for the Windows install check (scripts/check-windows-install.ps1).
# Imitates GitHub's `releases/latest` redirect and the `releases/download/<tag>/<file>` layout.
# GET/HEAD /latest answers 302 with Location: /tag/v<version>; every other path is a static file under --root.
# Serves on 127.0.0.1 only; logs requests to stderr; stdlib only, runs on Python 3.8+ (macOS, Linux, Windows).
# Usage: python scripts/windows-fixture-server.py --root DIR --port N --version X.Y.Z
import argparse
import functools
import http.server
import urllib.parse


class FixtureHandler(http.server.SimpleHTTPRequestHandler):
    def _redirect_latest(self):
        version = self.server.fixture_version
        self.send_response(302)
        self.send_header("Location", "/tag/v" + version)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):
        if urllib.parse.urlsplit(self.path).path == "/latest":
            self._redirect_latest()
            return
        super().do_GET()

    def do_HEAD(self):
        if urllib.parse.urlsplit(self.path).path == "/latest":
            self._redirect_latest()
            return
        super().do_HEAD()


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description="Local GitHub-release-like fixture server.")
    parser.add_argument("--root", required=True, help="directory to serve static files from")
    parser.add_argument("--port", required=True, type=int, help="TCP port to bind on 127.0.0.1")
    parser.add_argument("--version", required=True, help="version X.Y.Z used for the /latest redirect")
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    handler = functools.partial(FixtureHandler, directory=args.root)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), handler)
    server.fixture_version = args.version
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
