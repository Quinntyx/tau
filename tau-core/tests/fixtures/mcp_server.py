import json
import sys

def send(value):
    body = json.dumps(value, separators=(",", ":")).encode()
    sys.stdout.buffer.write(body + b"\n")
    sys.stdout.buffer.flush()

while True:
    line = sys.stdin.buffer.readline()
    if not line:
        break
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0", "id":request["id"], "result":{
            "protocolVersion":"2025-06-18", "capabilities":{"tools":{},"prompts":{}},
            "serverInfo":{"name":"tau-fixture", "version":"1"}}})
    elif method == "tools/list":
        send({"jsonrpc":"2.0", "id":request["id"], "result":{"tools":[{
            "name":"fixture_echo", "description":"deterministic fixture",
                "inputSchema":{"type":"object"}},
                {"name":"fixture_env", "description":"returns configured environment",
                 "inputSchema":{"type":"object"}}]}})
    elif method == "prompts/list":
        send({"jsonrpc":"2.0", "id":request["id"], "result":{"prompts":[{
            "name":"fixture_prompt", "description":"deterministic fixture prompt"}]}})
    elif method == "prompts/get":
        send({"jsonrpc":"2.0", "id":request["id"], "result":{
            "description":"deterministic fixture prompt",
            "messages":[{"role":"user", "content":{"type":"text", "text":"fixture prompt"}}]}})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        if name == "fixture_crash":
            sys.exit(17)
        value = params.get("arguments", {}).get("value", "")
        if name == "fixture_env":
            value = __import__("os").environ.get("TAU_FIXTURE_VALUE", "")
        send({"jsonrpc":"2.0", "id":request["id"], "result":{
            "content":[{"type":"text", "text":value}], "isError":False}})
