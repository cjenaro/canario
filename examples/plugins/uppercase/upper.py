#!/usr/bin/env python3
"""Canario hello-world plugin: uppercases every transcript.

The whole plugin contract in ~8 lines: read one NDJSON request per
line (`{"id", "hook", "text"}`), reply with the same `id` and the
transformed `text`, flush, repeat. The sidecar spawns this once, keeps
it resident, and enforces the per-plugin deadline — a slow or crashing
plugin degrades alone; dictation never blocks (canario-11h.1 D5d).

Install: copy this directory to ~/.config/canario/plugins/uppercase/
and add "uppercase" to plugins.enabled (+ plugins.enabled_master) in
config.json.
"""
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    reply = {"id": request["id"], "text": request["text"].upper()}
    print(json.dumps(reply), flush=True)
