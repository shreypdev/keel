#!/usr/bin/env python3
"""Serves a directory on 127.0.0.1 at a free port, as GitHub serves a release's assets, for the packaging tests
(packaging/test-npm-assets.sh, packaging/rehearse-launch.sh).

`python3 -m http.server` looks the host's name up first (socket.getfqdn), which on a macOS CI runner can take longer
than the tests wait before it says where it listens; this server skips that lookup. It prints
`Serving HTTP on 127.0.0.1 port <n>` once it listens, and a line per request on stderr.

    python3 -u packaging/serve-dir.py <dir>
"""

import functools
import http.server
import socketserver
import sys


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True

    def server_bind(self):
        # HTTPServer.server_bind would resolve the address's name (socket.getfqdn); the handler does not need it.
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit("usage: serve-dir.py <dir>")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[1])
    with Server(("127.0.0.1", 0), handler) as httpd:
        print(f"Serving HTTP on 127.0.0.1 port {httpd.server_address[1]}", flush=True)
        httpd.serve_forever()


if __name__ == "__main__":
    main()
