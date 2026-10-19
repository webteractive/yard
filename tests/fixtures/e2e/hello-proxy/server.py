"""The app behind `yard proxy hello-proxy`: answers every GET with "proxied"."""

import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


class Proxied(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"proxied\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


HTTPServer(("127.0.0.1", int(sys.argv[1])), Proxied).serve_forever()
