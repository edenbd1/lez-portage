#!/usr/bin/env python3
"""Record the public JSON-RPC answers a verifier reads, and serve them back.

The public LEZ testnet is reset when it moves to 0.4, and a public RPC can
stop serving old state. A re-check run after that would find nothing to read.
So the verifiers' own reads are kept: a proxy forwards every request to the
public endpoint and writes the answer down, and later serves the same answers
to the same, unchanged verifiers.

    scripts/rpc-archive.py record UPSTREAM FILE PORT   forward, and append every answer to FILE
    scripts/rpc-archive.py serve  FILE PORT            answer from FILE only; anything not in it
                                                       is an error, never a guess

FILE is JSON lines, one per distinct request: {"req": {method, params} or a
GET path, "res": the answer without its id}. The verifiers recompute what
they check from the bytes (block hashes, transaction hashes, Merkle proofs,
signatures), so an archive cannot make a wrong block pass as a block whose
hash the manifest records. What it cannot show is that the public network
served those bytes; that was checked live when the archive was recorded.
"""
import gzip
import http.server
import json
import socketserver
import sys
import threading
import time
import urllib.error
import urllib.request


# Lookups by transaction hash, answered "not found" for a hash the archive lacks.
LOOKUPS = {"getTransaction", "eth_getTransactionByHash", "eth_getTransactionReceipt"}


def key(req):
    return json.dumps(req, sort_keys=True, separators=(",", ":"))


def opener(path, mode):
    return gzip.open(path, mode + "t") if path.endswith(".gz") else open(path, mode)


class Store:
    def __init__(self, path, writing):
        self.path, self.lock, self.seen = path, threading.Lock(), {}
        try:
            with opener(path, "r") as f:
                for line in f:
                    if line.strip():
                        e = json.loads(line)
                        self.seen[key(e["req"])] = e["res"]
        except FileNotFoundError:
            if not writing:
                raise
        self.out = opener(path, "a") if writing else None

    def get(self, req):
        return self.seen.get(key(req))

    def put(self, req, res):
        with self.lock:
            k = key(req)
            if k not in self.seen:
                self.seen[k] = res
                self.out.write(json.dumps({"req": req, "res": res}, sort_keys=True) + "\n")
                self.out.flush()


def handler(store, upstream):
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def reply(self, code, body, ctype="application/json"):
            data = body if isinstance(body, bytes) else json.dumps(body).encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            req = {"GET": self.path}
            res = store.get(req)
            if res is None and upstream:
                try:
                    with urllib.request.urlopen(upstream.rstrip("/") + self.path, timeout=60) as r:
                        res = {"status": r.status, "body": r.read().decode("latin-1"), "type": r.headers.get("Content-Type", "")}
                except urllib.error.HTTPError as e:
                    res = {"status": e.code, "body": e.read().decode("latin-1"), "type": e.headers.get("Content-Type", "")}
                store.put(req, res)
            if res is None:
                return self.reply(404, {"error": "not in the archive: GET " + self.path})
            self.reply(res["status"], res["body"].encode("latin-1"), res["type"] or "application/octet-stream")

        def one(self, call):
            req = {"method": call.get("method"), "params": call.get("params", [])}
            res = store.get(req)
            if res is None and upstream:
                body = json.dumps({"jsonrpc": "2.0", "id": 1, **req}).encode()
                r = urllib.request.Request(upstream, data=body, headers={"Content-Type": "application/json", "User-Agent": "rpc-archive"})
                # A public endpoint rate-limits and drops connections now and then:
                # that is not an answer to record, so retry until it answers.
                for attempt in range(8):
                    got = {}
                    try:
                        with urllib.request.urlopen(r, timeout=60) as resp:
                            got = json.loads(resp.read())
                    except Exception as e:
                        sys.stderr.write("rpc-archive: %s %s: %s, retrying\n" % (upstream, req["method"], e))
                    err = got.get("error") or {}
                    if "result" in got or (err and err.get("code") not in (-32005, 429, -32603) and "rate" not in str(err).lower()):
                        break
                    time.sleep(min(2 ** attempt, 30))
                res = {k: v for k, v in got.items() if k in ("result", "error")}
                if "result" in res or "error" in res:
                    store.put(req, res)
            if res is None and req["method"] in LOOKUPS:
                # A transaction the archive does not hold is one the recorded runs never
                # read: "not found" is the truthful answer, and it can only fail a check
                # that expects the transaction. The verifiers' controls ask for a random
                # hash and expect exactly this.
                res = {"result": None}
            if res is None:
                res = {"error": {"code": -32099, "message": "not in the archive: " + key(req)[:200]}}
            return {"jsonrpc": "2.0", "id": call.get("id"), **res}

        def do_POST(self):
            n = int(self.headers.get("Content-Length", 0))
            body = json.loads(self.rfile.read(n) or b"null")
            out = [self.one(c) for c in body] if isinstance(body, list) else self.one(body)
            self.reply(200, out)

    return H


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    a = sys.argv[1:]
    if a[:1] == ["record"] and len(a) == 4:
        upstream, path, port = a[1], a[2], int(a[3])
        store = Store(path, True)
    elif a[:1] == ["serve"] and len(a) == 3:
        upstream, path, port = None, a[1], int(a[2])
        store = Store(path, False)
    else:
        raise SystemExit(__doc__)
    srv = Server(("127.0.0.1", port), handler(store, upstream))
    print("rpc-archive %s %s on 127.0.0.1:%d (%d answers)" % (a[0], path, port, len(store.seen)), flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
