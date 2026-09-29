#!/usr/bin/env python3
"""Black-box MCP acceptance; same assertions over local pipes or real OpenSSH."""
import argparse
import json
import os
import queue
import shlex
import subprocess
import tempfile
import threading
import time


class Client:
    def __init__(self, command):
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, bufsize=1)
        self.messages = queue.Queue()
        self.next_id = 0
        self.pending = {}
        self.stderr = []

        def read():
            try:
                for line in self.process.stdout:
                    self.messages.put(json.loads(line))
            except Exception as exc:
                self.messages.put(exc)
            finally:
                self.messages.put(EOFError("server stdout closed"))

        def errors():
            self.stderr.extend(self.process.stderr)

        threading.Thread(target=read, daemon=True).start()
        threading.Thread(target=errors, daemon=True).start()

    def send(self, method, params=None, notification=False):
        message = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            message["params"] = params
        if not notification:
            self.next_id += 1
            message["id"] = self.next_id
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()
        return message.get("id")

    def receive(self, request_id):
        deadline = time.monotonic() + 15
        while request_id not in self.pending:
            message = self.messages.get(timeout=max(0.01, deadline - time.monotonic()))
            if isinstance(message, Exception):
                raise message
            if "id" in message:
                self.pending[message["id"]] = message
        return self.pending.pop(request_id)

    def rpc(self, method, params=None):
        response = self.receive(self.send(method, params))
        assert "error" not in response, response
        return response["result"]

    def operation(self, op, workspace="main", error=None, **kwargs):
        result = self.rpc("tools/call", {"name": "workspace_execute", "arguments": {
            "workspace": workspace, "operation": {"op": op, **kwargs}}})
        value = result["structuredContent"]
        assert json.loads(result["content"][0]["text"]) == value
        if error:
            assert result["isError"] and value["error"]["code"] == error, result
        else:
            assert not result.get("isError"), result
        return value

    def close(self):
        self.process.stdin.close()
        try:
            assert self.process.wait(timeout=10) == 0, self.stderr
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()


def exercise(client):
    init = client.rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                    "clientInfo": {"name": "portspace-acceptance", "version": "1"}})
    assert "tools" in init["capabilities"]
    client.send("notifications/initialized", notification=True)
    assert client.rpc("ping") == {}
    tools = client.rpc("tools/list")["tools"]
    assert {t["name"] for t in tools} == {"workspace_list", "workspace_execute"}
    assert all(t["inputSchema"]["type"] == "object" for t in tools)
    discovery = client.rpc("tools/call", {"name": "workspace_list", "arguments": {}})["structuredContent"]
    assert {w["id"] for w in discovery["workspaces"]} == {"main", "other"}
    assert all(w["root"] == "." for w in discovery["workspaces"])
    op = client.operation
    op("mkdir", path="src/nested")
    op("write", path="src/a file.txt", content="alpha\r\nbeta\r\n", encoding="utf8")
    op("edit", path="src/a file.txt", replacements=[{"old_text": "alpha", "new_text": "gamma"},
                                                    {"old_text": "beta", "new_text": "delta"}])
    assert op("read", path="src/a file.txt", encoding="utf8")["content"] == "gamma\r\ndelta\r\n"
    process = dict(program="/bin/sh", args=["-c", "cat 'src/a file.txt'; printf %s \"$PORTSPACE_TEST\" >&2"],
                   cwd=".", env={"PORTSPACE_TEST": "remote-env"}, timeout_ms=3000)
    result = op("exec", **process)
    assert result["stdout"] == "gamma\r\ndelta\r\n" and result["stderr"] == "remote-env"
    assert result["exit_code"] == 0
    op("stat", workspace="other", path="src/a file.txt", error="not_found")
    op("stat", workspace="absent", path=".", error="workspace_unavailable")
    for path in ["../outside", "/tmp"]:
        op("read", path=path, encoding="utf8", error="invalid_path")
    op("write", path="binary", content="AP8=", encoding="base64")
    assert op("read", path="binary", encoding="base64")["content"] == "AP8="
    op("read", path="binary", encoding="utf8", error="invalid_input")
    op("edit", path="src/a file.txt", replacements=[{"old_text": "gamma", "new_text": "oops"},
        {"old_text": "absent", "new_text": "x"}], error="conflict")
    assert op("read", path="src/a file.txt", encoding="utf8")["content"].startswith("gamma")
    search = op("search", path="src", pattern="gamma|delta", limit=1)
    assert search["truncated"] and search["matches"][0]["line"] == 1
    assert len(op("list", path="src")["entries"]) == 2
    op("exec", **{**process, "args": ["-c", "ln -s /tmp link"]})
    op("stat", path="link", error="invalid_path")
    result = op("exec", **{**process, "args": ["-c", "exit 9"]})
    assert result["exit_code"] == 9
    result = op("exec", **{**process, "args": ["-c", "head -c 1100000 /dev/zero"]})
    assert result["stdout_truncated"] and len(result["stdout"]) == 1048576
    op("exec", **{**process, "args": ["-c", "(sleep 1; touch timeout-leak) & wait"], "timeout_ms": 50}, error="timeout")
    cancel_id = client.send("tools/call", {"name": "workspace_execute", "arguments": {
        "workspace": "main", "operation": {"op": "exec", **process,
        "args": ["-c", "touch started; (sleep 1; touch cancel-leak) & wait"], "timeout_ms": 5000}}})
    time.sleep(0.3)
    client.send("notifications/cancelled", {"requestId": cancel_id, "reason": "acceptance test"}, notification=True)
    time.sleep(1.2)
    op("stat", path="started")
    op("stat", path="timeout-leak", error="not_found")
    op("stat", path="cancel-leak", error="not_found")
    op("remove", path="src", recursive=True)
    op("stat", path="src", error="not_found")
    op("remove", path=".", recursive=True, error="invalid_path")
    unknown = client.receive(client.send("tools/call", {"name": "absent", "arguments": {}}))
    assert "error" in unknown
    invalid = client.receive(client.send("tools/call", {"name": "workspace_execute", "arguments": {}}))
    assert "error" in invalid or invalid.get("result", {}).get("isError")
    print("PASS: handshake, discovery, schemas, file/edit/process coherence, isolation, binary, errors, "
          "search, output limits, timeout, cancellation, lifecycle")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--ssh", help="OpenSSH host alias; binary must be an absolute remote path")
    args = parser.parse_args()
    root = None
    client = None
    try:
        if args.ssh:
            root = subprocess.check_output(["ssh", "-T", "-o", "BatchMode=yes", args.ssh,
                                            "mktemp -d /tmp/portspace-e2e.XXXXXXXX"], text=True).strip()
            assert root.startswith("/tmp/portspace-e2e.") and "\n" not in root
            subprocess.run(["ssh", "-T", args.ssh, "mkdir " + shlex.quote(root + "/other")], check=True)
        else:
            root = tempfile.mkdtemp(prefix="portspace-e2e.")
            os.mkdir(root + "/other")
        command = [args.binary, "--workspace", "main=" + root, "--workspace", "other=" + root + "/other"]
        if args.ssh:
            command = ["ssh", "-T", "-o", "BatchMode=yes", args.ssh, shlex.join(command)]
        client = Client(command)
        exercise(client)
        client.send("tools/call", {"name": "workspace_execute", "arguments": {
            "workspace": "main", "operation": {"op": "exec", "program": "/bin/sh",
            "args": ["-c", "touch eof-started; (sleep 1; touch eof-leak) & wait"],
            "cwd": ".", "env": {}, "timeout_ms": 5000}}})
        time.sleep(0.3)
        client.close()
        client = None
        # Session closure preserves files but must cancel active process groups.
        time.sleep(1.2)
        if args.ssh:
            checks = " && ".join(["test -f " + shlex.quote(root + "/binary"),
                                   "test -f " + shlex.quote(root + "/eof-started"),
                                   "test ! -e " + shlex.quote(root + "/eof-leak")])
            subprocess.run(["ssh", "-T", args.ssh, checks], check=True)
        else:
            assert os.path.isfile(root + "/binary")
            assert os.path.isfile(root + "/eof-started")
            assert not os.path.exists(root + "/eof-leak")
        print("PASS: EOF cancels processes and preserves workspace; transport=" + (args.ssh or "local"))
    finally:
        if client:
            try:
                client.close()
            except Exception:
                print("server stderr:", "".join(client.stderr))
        if root:
            if args.ssh:
                subprocess.run(["ssh", "-T", args.ssh, "rm -rf -- " + shlex.quote(root)], check=True)
            else:
                import shutil
                shutil.rmtree(root)


if __name__ == "__main__":
    main()
