"""Controlled stdio MCP server for ara-agent-lite-mcp tests.

Options (argv):
  --protocol V        protocolVersion returned by initialize (default 2025-06-18)
  --init-delay S      seconds to wait before answering initialize
  --exit-at-start     print FAKE_SECRET (env) to stderr and exit before initialize
  --stderr-flood N    write N bytes of noise to stderr at startup
  --page-size N       tools/list page size (default: all tools in one page)
  --endless-pages     tools/list always returns nextCursor
  --no-tools          do not declare the tools capability
  --resources N       declare resources with N resources (page size 2)
  --instructions T    initialize instructions
  --grandchild FILE   start a sleeping grandchild process and write its PID to FILE
  --naming            add tools that exercise name normalization and exclusion
Env:
  FAKE_MCP_LOG        append one line per received request/notification
Tools: echo, slow, slow_ro (readOnlyHint), die, fail, big, image, structured,
resource, ask_client, env.
"""
import json
import os
import subprocess
import sys
import threading
import time

args = sys.argv[1:]


def opt(name, default=None):
    if name in args:
        index = args.index(name)
        return args[index + 1] if index + 1 < len(args) else default
    return default


def flag(name):
    return name in args


LOG = os.environ.get("FAKE_MCP_LOG")
write_lock = threading.Lock()
log_lock = threading.Lock()
replies = {}
replies_ready = threading.Condition()


def log(line):
    if not LOG:
        return
    with log_lock:
        with open(LOG, "a", encoding="utf-8") as handle:
            handle.write(line + "\n")
            handle.flush()


def send(message):
    data = json.dumps(message) + "\n"
    with write_lock:
        sys.stdout.write(data)
        sys.stdout.flush()


def result(request_id, value):
    send({"jsonrpc": "2.0", "id": request_id, "result": value})


def text(value):
    return {"content": [{"type": "text", "text": value}]}


OBJ = {"type": "object", "properties": {}}
TOOLS = [
    {"name": "echo", "description": "Echo text.", "annotations": {"readOnlyHint": True},
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
    {"name": "slow", "description": "Sleep ms then answer.",
     "inputSchema": {"type": "object", "properties": {"ms": {"type": "integer"}}}},
    {"name": "slow_ro", "description": "Read-only sleep.", "annotations": {"readOnlyHint": True},
     "inputSchema": {"type": "object", "properties": {"ms": {"type": "integer"}}}},
    {"name": "die", "description": "Exit without reply after ms.",
     "inputSchema": {"type": "object", "properties": {"ms": {"type": "integer"}}}},
    {"name": "die_ro", "description": "Read-only exit without reply.", "annotations": {"readOnlyHint": True},
     "inputSchema": {"type": "object", "properties": {"ms": {"type": "integer"}}}},
    {"name": "fail", "description": "isError with n characters.",
     "inputSchema": {"type": "object", "properties": {"n": {"type": "integer"}}}},
    {"name": "big", "description": "Text with n characters.",
     "inputSchema": {"type": "object", "properties": {"n": {"type": "integer"}}}},
    {"name": "image", "description": "Image content.", "inputSchema": OBJ},
    {"name": "structured", "description": "Structured content.", "inputSchema": OBJ},
    {"name": "resource", "description": "Embedded resource.", "inputSchema": OBJ},
    {"name": "ask_client", "description": "Send server requests to the client.", "inputSchema": OBJ},
    {"name": "env", "description": "Report an environment variable.",
     "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}}},
]
if flag("--naming"):
    TOOLS += [
        {"name": "weird name!", "description": "Spaces and punctuation.", "inputSchema": OBJ},
        {"name": "dup.x", "description": "First of a normalized pair.", "inputSchema": OBJ},
        {"name": "dup_x", "description": "Second of a normalized pair.", "inputSchema": OBJ},
        {"name": "x" * 130, "description": "Too long.", "inputSchema": OBJ},
        {"name": "not_object", "description": "Bad schema.", "inputSchema": {"type": "string"}},
        {"name": "choice", "description": "Either a or b+c.",
         "inputSchema": {"type": "object", "oneOf": [
             {"properties": {"a": {"type": "string"}}, "required": ["a"]},
             {"properties": {"b": {"type": "string"}, "c": {"type": "string"}}, "required": ["b", "c"]},
         ]}},
    ]
RESOURCES = [
    {"uri": "fake://doc/%d" % i, "name": "doc %d" % i, "mimeType": "text/plain"}
    for i in range(int(opt("--resources", "0") or 0))
]


def ask_client(request_id):
    asked = []
    for index, method in enumerate(["sampling/createMessage", "roots/list", "elicitation/create", "ping"]):
        rid = "srv-%d" % index
        asked.append(rid)
        send({"jsonrpc": "2.0", "id": rid, "method": method, "params": {}})
    deadline = time.time() + 5
    with replies_ready:
        while not all(r in replies for r in asked) and time.time() < deadline:
            replies_ready.wait(0.1)
    codes = []
    for rid in asked:
        reply = replies.get(rid)
        if reply is None:
            codes.append("none")
        elif "error" in reply:
            codes.append(str(reply["error"].get("code")))
        else:
            codes.append("ok")
    result(request_id, text("codes:" + ",".join(codes)))


def call(request_id, params):
    name = params.get("name")
    arguments = params.get("arguments") or {}
    log("call %s %s" % (name, request_id))
    if name == "echo":
        result(request_id, text(arguments.get("text", "")))
    elif name in ("slow", "slow_ro"):
        time.sleep(int(arguments.get("ms", 1000)) / 1000.0)
        log("finished %s %s" % (name, request_id))
        result(request_id, text("done"))
    elif name in ("die", "die_ro"):
        time.sleep(int(arguments.get("ms", 0)) / 1000.0)
        os._exit(3)
    elif name == "fail":
        n = int(arguments.get("n", 10))
        result(request_id, {"content": [{"type": "text", "text": "e" * n}], "isError": True})
    elif name == "big":
        n = int(arguments.get("n", 10))
        result(request_id, text("b" * n))
    elif name == "image":
        result(request_id, {"content": [{"type": "text", "text": "an image"},
                                        {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}]})
    elif name == "structured":
        result(request_id, {"content": [{"type": "text", "text": "{\"answer\": 42}"}],
                            "structuredContent": {"answer": 42}})
    elif name == "resource":
        result(request_id, {"content": [
            {"type": "resource", "resource": {"uri": "fake://r", "mimeType": "text/plain", "text": "inside"}},
            {"type": "resource_link", "uri": "fake://link", "name": "link"}]})
    elif name == "ask_client":
        ask_client(request_id)
    elif name == "env":
        result(request_id, text(os.environ.get(arguments.get("name", ""), "<unset>")))
    else:
        send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32602, "message": "unknown tool " + str(name)}})


def handle(message):
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params") or {}
    if method == "initialize":
        log("initialize " + str(params.get("protocolVersion")))
        delay = float(opt("--init-delay", "0") or 0)
        if delay:
            time.sleep(delay)
        capabilities = {}
        if not flag("--no-tools"):
            capabilities["tools"] = {}
        if RESOURCES:
            capabilities["resources"] = {}
        answer = {"protocolVersion": opt("--protocol", "2025-06-18"), "capabilities": capabilities,
                  "serverInfo": {"name": "fake", "version": "1"}}
        if opt("--instructions"):
            answer["instructions"] = opt("--instructions")
        result(request_id, answer)
    elif method == "tools/list":
        cursor = int(params.get("cursor", "0"))
        log("tools/list %d" % cursor)
        size = int(opt("--page-size", "0") or 0) or len(TOOLS)
        page = TOOLS[cursor:cursor + size]
        answer = {"tools": page}
        if flag("--endless-pages") or cursor + size < len(TOOLS):
            answer["nextCursor"] = str(cursor + size)
        result(request_id, answer)
    elif method == "tools/call":
        call(request_id, params)
    elif method == "resources/list":
        cursor = int(params.get("cursor", "0"))
        answer = {"resources": RESOURCES[cursor:cursor + 2]}
        if cursor + 2 < len(RESOURCES):
            answer["nextCursor"] = str(cursor + 2)
        result(request_id, answer)
    elif method == "resources/read":
        uri = params.get("uri")
        result(request_id, {"contents": [{"uri": uri, "mimeType": "text/plain", "text": "content of " + str(uri)}]})
    else:
        send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32601, "message": "Method not found"}})


def main():
    flood = int(opt("--stderr-flood", "0") or 0)
    if flood:
        sys.stderr.write("n" * (flood - 4) + "END\n")
        sys.stderr.flush()
    if flag("--exit-at-start"):
        sys.stderr.write("fatal: cannot start, secret=%s\n" % os.environ.get("FAKE_SECRET", ""))
        sys.stderr.flush()
        sys.exit(2)
    pid_file = opt("--grandchild")
    if pid_file:
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"])
        with open(pid_file, "w", encoding="utf-8") as pid_out:
            pid_out.write(str(child.pid))
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        if "method" not in message:
            with replies_ready:
                replies[message.get("id")] = message
                replies_ready.notify_all()
            continue
        if "id" not in message:
            log("notification %s %s" % (message["method"], json.dumps(message.get("params"), sort_keys=True)))
            continue
        threading.Thread(target=handle, args=(message,), daemon=True).start()
    log("stdin closed")


main()
