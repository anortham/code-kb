"""Stdio MCP server that logs how a client reports its project directory.

Usage: python3 mcp-probe.py [LOG_DIR]   (default ~/.code-kb/probe)

Each process appends JSON lines to LOG_DIR/<pid>.jsonl: its start state, every
message in and out, and one `whereami` record per tool call. The `whereami` tool
polls `roots/list` when the client declares the roots capability.
"""
import json, os, queue, sys, threading, time

LOG_DIR = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/.code-kb/probe")
os.makedirs(LOG_DIR, exist_ok=True)
LOG = os.path.join(LOG_DIR, f"{os.getpid()}.jsonl")
ENV_HINTS = ("PWD", "CLAUDE", "CODEX", "GROK", "AGY", "ANTIGRAVITY", "GEMINI", "CURSOR", "WORKSPACE", "PROJECT")
ROOTS_TIMEOUT_S = 2.0

inbox = queue.Queue()
held = []
state = {"client": None, "protocol": None, "roots_capable": False, "list_changed": 0, "next_id": 0}


def log(tag, obj):
    with open(LOG, "a") as f:
        f.write(json.dumps({"t": round(time.time(), 3), "tag": tag, "msg": obj}) + "\n")


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
    log("out", obj)


def read_stdin():
    for line in sys.stdin:
        if line.strip():
            inbox.put(json.loads(line))
    inbox.put(None)


def receive(timeout=None):
    msg = inbox.get(timeout=timeout)
    if msg is None:
        sys.exit(0)
    log("in", msg)
    return msg


def poll_roots():
    if not state["roots_capable"]:
        return {"roots": "client declared no roots capability"}
    state["next_id"] += 1
    want = f"probe-{state['next_id']}"
    started = time.time()
    send({"jsonrpc": "2.0", "id": want, "method": "roots/list"})
    deadline = started + ROOTS_TIMEOUT_S
    while True:
        try:
            msg = receive(max(0.0, deadline - time.time()))
        except queue.Empty:
            return {"roots": "timeout", "wait_ms": round((time.time() - started) * 1000, 1)}
        if msg.get("id") == want and "method" not in msg:
            answer = msg.get("result", msg.get("error"))
            return {"roots": answer, "wait_ms": round((time.time() - started) * 1000, 1)}
        held.append(msg)


def whereami(arguments):
    report = {
        "client": state["client"],
        "protocol": state["protocol"],
        "cwd": os.getcwd(),
        "arguments": arguments,
        "list_changed_seen": state["list_changed"],
        **poll_roots(),
    }
    log("whereami", report)
    return json.dumps(report, indent=1)


def handle(msg):
    method, msg_id = msg.get("method"), msg.get("id")
    if method == "initialize":
        params = msg.get("params", {})
        state["client"] = params.get("clientInfo")
        state["protocol"] = params.get("protocolVersion")
        state["roots_capable"] = "roots" in params.get("capabilities", {})
        send({"jsonrpc": "2.0", "id": msg_id, "result": {
            "protocolVersion": state["protocol"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "mcp-probe", "version": "1"},
            "instructions": "Call whereami when asked where the MCP server thinks the project is.",
        }})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": [{
            "name": "whereami",
            "description": "Report the directory this MCP server sees as the project: cwd and client roots. "
                           "Pass an optional absolute path to echo it back.",
            "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}},
        }]}})
    elif method == "tools/call":
        text = whereami(msg.get("params", {}).get("arguments", {}))
        send({"jsonrpc": "2.0", "id": msg_id, "result": {"content": [{"type": "text", "text": text}]}})
    elif method == "notifications/roots/list_changed":
        state["list_changed"] += 1
    elif method == "ping":
        send({"jsonrpc": "2.0", "id": msg_id, "result": {}})
    elif method and msg_id is not None:
        send({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": f"unknown method {method}"}})


log("start", {
    "pid": os.getpid(), "ppid": os.getppid(), "cwd": os.getcwd(), "argv": sys.argv,
    "env": {k: v for k, v in os.environ.items()
            if any(h in k for h in ENV_HINTS) and not any(s in k for s in ("TOKEN", "KEY", "SECRET"))},
})
threading.Thread(target=read_stdin, daemon=True).start()
while True:
    handle(held.pop(0) if held else receive())
