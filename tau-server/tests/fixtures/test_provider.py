import json
import sys


def send(value, line_mode):
    body = json.dumps(value, separators=(",", ":")).encode()
    if line_mode:
        sys.stdout.buffer.write(body + b"\n")
    else:
        sys.stdout.buffer.write(
            ("Content-Length: %d\r\n\r\n" % len(body)).encode() + body
        )
    sys.stdout.buffer.flush()


while True:
    line = sys.stdin.buffer.readline()
    if not line:
        break
    line_mode = line.lstrip().startswith(b"{")
    if line_mode:
        body = line
    else:
        headers = {}
        while line not in (b"\r\n", b"\n", b""):
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
            line = sys.stdin.buffer.readline()
        if not line:
            break
        body = sys.stdin.buffer.read(int(headers["content-length"]))

    request = json.loads(body)
    method = request.get("method", "")
    result = {}
    if method == "initialize":
        result = {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}, "prompts": {}},
            "serverInfo": {"name": "tau-test-provider", "version": "1"},
        }
    elif method == "tools/list":
        result = {
            "tools": [
                {
                    "name": "fixture_echo",
                    "description": "deterministic echo",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"value": {"type": "string"}},
                        "required": ["value"],
                    },
                }
            ]
        }
    elif method == "prompts/list":
        result = {"prompts": []}
    elif method == "tools/call":
        value = request.get("params", {}).get("arguments", {}).get("value", "")
        result = {"content": [{"type": "text", "text": value}], "isError": False}
    elif method.startswith("textDocument/"):
        result = [
            {
                "uri": "file:///fixture.rs",
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 1},
                },
            }
        ]

    if "id" in request:
        send(
            {"jsonrpc": "2.0", "id": request.get("id"), "result": result},
            line_mode,
        )
