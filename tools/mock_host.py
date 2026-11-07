"""Mock image host for testing the upload path.

Run it, then set the endpoint in MeowDown's settings to
`http://127.0.0.1:36678/upload` and use "测试上传一张图" or Ctrl+Shift+U.

    python tools/mock_host.py
    cargo test -- --ignored
"""
import json
import socketserver
from http.server import BaseHTTPRequestHandler


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length)
        path = self.path
        size = len(body)
        print(f"POST {path} {size} bytes {self.headers.get('Content-Type')}")
        url = f"https://cdn.example.com/uploaded/{size}.png"
        payload = json.dumps(
            {"success": True, "result": [url], "data": {"url": url}}
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        pass


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True


if __name__ == "__main__":
    with Server(("127.0.0.1", 36678), Handler) as server:
        print("listening on 127.0.0.1:36678")
        server.serve_forever()
