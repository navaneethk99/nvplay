#!/usr/bin/env python3
"""Deallocate this VM only after the signaling service grants an idle lease."""
import argparse
import json
import sys
from urllib.request import Request, urlopen
from urllib.error import URLError


def request_json(url, *, headers=None, method="GET"):
    request = Request(url, headers=headers or {}, method=method)
    with urlopen(request, timeout=20) as response:
        data = response.read()
        return json.loads(data) if data else {}


def run(config, check=False):
    local = config.get("service", "http://127.0.0.1:8787")
    headers = {"Authorization": "Bearer " + config["host_key"]}
    state = request_json(local + ("/admin/activity" if check else "/admin/idle"),
                         headers=headers, method="GET" if check else "POST")
    if check:
        print(json.dumps(state))
        return
    if not state.get("shutdown"):
        return
    # Azure-managed identity; no client secret or owner credentials on the VM.
    token = request_json(
        "http://169.254.169.254/metadata/identity/oauth2/token"
        "?api-version=2018-02-01&resource=https%3A%2F%2Fmanagement.azure.com%2F",
        headers={"Metadata": "true"})["access_token"]
    request_json("https://management.azure.com" + config["vm_id"] +
                 "/deallocate?api-version=2024-03-01",
                 headers={"Authorization": "Bearer " + token}, method="POST")
    print("Idle watch service: Azure deallocation requested.", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", default="/etc/nvplay-watch-idle.json")
    parser.add_argument("--check", action="store_true", help="Read status without claiming shutdown")
    args = parser.parse_args()
    try:
        with open(args.config, encoding="utf-8") as source:
            run(json.load(source), args.check)
    except (OSError, URLError, ValueError, KeyError) as error:
        # Do not print request headers, bearer credentials, or response bodies.
        print("Idle check failed (" + type(error).__name__ + "); retrying next minute.", file=sys.stderr)
        sys.exit(1)
