#!/usr/bin/env python3
"""Opt-in live verification of the Hardcore Grand Exchange price API.

Not part of CI or scripts/check.sh: it makes real network requests, so run it
deliberately and by hand.

    python3 scripts/verify_hardcore_api.py            # five Hardcore requests
    python3 scripts/verify_hardcore_api.py --compare-standard   # plus one standard-world request
    python3 scripts/verify_hardcore_api.py --self-test          # offline check of the summariser

Behaviour, kept deliberately small and polite:
  * identifies the project in the User-Agent, in the same format as the Rust
    client's default (project URL, no personal contact);
  * sequential requests with a pause between them, explicit timeouts and a
    response size cap;
  * prints status, selected headers and a compact schema summary per endpoint;
  * never writes response bodies to disk, so nothing bulky can be committed.

Python standard library only.
"""

import json
import re
import sys
import time
import urllib.error
import urllib.request
from collections import Counter
from pathlib import Path

BASE = "https://prices.runescape.wiki/api/v2/"


def crate_version():
    """The package version from Cargo.toml, so the User-Agent matches the client's."""
    manifest = Path(__file__).resolve().parent.parent / "Cargo.toml"
    match = re.search(r'^version\s*=\s*"([^"]+)"', manifest.read_text(), re.MULTILINE)
    return match.group(1) if match else "unknown"


# Same format as the Rust client's default (`ClientConfig::default`), so a
# successful run also shows that this User-Agent is accepted.
USER_AGENT = f"osrs-mcp/{crate_version()} (+https://github.com/whcas/OSRS-MCP)"
TIMEOUT_SECONDS = 30
MAX_BODY_BYTES = 8 * 1024 * 1024
PAUSE_SECONDS = 2.0
SAMPLE_ITEM = 4151
SHOWN_HEADERS = (
    "content-type", "content-length", "content-encoding", "cache-control", "age",
    "date", "last-modified", "etag", "retry-after", "x-ratelimit-limit",
    "x-ratelimit-remaining", "x-ratelimit-reset",
)
# The prices API reports Unix time in seconds. Millisecond values would be
# around 1.7e12; anything above this bound is flagged.
SECONDS_UPPER_BOUND = 4_000_000_000


def type_name(value):
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, int):
        return "int"
    if isinstance(value, float):
        return "float"
    return type(value).__name__


def field_types(entries):
    """Maps field name -> Counter of value types across a list of dicts."""
    types = {}
    for entry in entries:
        for key, value in entry.items():
            types.setdefault(key, Counter())[type_name(value)] += 1
    return {key: dict(counter) for key, counter in sorted(types.items())}


def decimals(value):
    if isinstance(value, float):
        text = repr(value)
        return len(text.split(".")[1]) if "." in text and "e" not in text else None
    return 0


def checks_for_window(entries, header):
    """Observations bearing on semantics the documentation does not state."""
    notes = []
    stats = [e for e in entries if isinstance(e, dict)]
    inconsistent = sum(
        1 for e in stats
        if (e.get("avgHighPrice") is None and e.get("highPriceVolume", 0) > 0)
        or (e.get("avgLowPrice") is None and e.get("lowPriceVolume", 0) > 0)
    )
    notes.append(f"entries with a null average but non-zero volume: {inconsistent}")
    over_two = sum(
        1 for e in stats for k in ("avgHighPrice", "avgLowPrice")
        if (decimals(e.get(k)) or 0) > 2
    )
    notes.append(f"averages with more than two decimals: {over_two}")
    fractional_volume = sum(
        1 for e in stats for k in ("highPriceVolume", "lowPriceVolume")
        if not isinstance(e.get(k), int)
    )
    notes.append(f"non-integer volumes: {fractional_volume}")
    timestamp = header.get("timestamp")
    if isinstance(timestamp, int):
        notes.append(
            f"timestamp {timestamp}: "
            f"{'looks like seconds' if timestamp < SECONDS_UPPER_BOUND else 'NOT seconds?'}"
            f", multiple of 300: {timestamp % 300 == 0}, multiple of 3600: {timestamp % 3600 == 0}"
        )
    return notes


def summarise(name, body, now):
    """Returns a list of summary lines for one decoded JSON body."""
    lines = []
    if name == "mapping":
        lines.append(f"top level: array, {len(body)} entries")
        lines.append(f"field types: {json.dumps(field_types(body))}")
    elif name == "latest":
        data = body["data"]
        lines.append(f"top level keys: {sorted(body)}; items: {len(data)}")
        lines.append(f"field types: {json.dumps(field_types(data.values()))}")
        for item, entry in list(data.items())[:1]:
            lines.append(f"sample entry {item}: {json.dumps(entry)}")
            for key in ("highTime", "lowTime"):
                if isinstance(entry.get(key), int):
                    lines.append(
                        f"{key} age: {now - entry[key]} s "
                        f"({'seconds' if entry[key] < SECONDS_UPPER_BOUND else 'NOT seconds?'})"
                    )
    elif name in ("5m", "1h"):
        data = body["data"]
        header = {k: v for k, v in body.items() if k != "data"}
        lines.append(f"top level keys: {sorted(body)}; header: {json.dumps(header)}; items: {len(data)}")
        lines.append(f"field types: {json.dumps(field_types(data.values()))}")
        lines.extend(checks_for_window(list(data.values()), header))
    elif name == "timeseries":
        points = body["data"]
        header = {k: v for k, v in body.items() if k != "data"}
        lines.append(f"top level keys: {sorted(body)}; header: {json.dumps(header)}; points: {len(points)}")
        lines.append(f"point field types: {json.dumps(field_types(points))}")
        stamps = [p["timestamp"] for p in points]
        gaps = Counter(b - a for a, b in zip(stamps, stamps[1:]))
        lines.append(f"step gaps (seconds: count): {dict(gaps)}; header timestep: {body.get('timestep')}")
        lines.extend(checks_for_window(points, {}))
    return lines


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
            raw = response.read(MAX_BODY_BYTES + 1)
            status, headers = response.status, dict(response.headers.items())
    except urllib.error.HTTPError as error:
        raw, status, headers = error.read(MAX_BODY_BYTES + 1), error.code, dict(error.headers.items())
    return status, headers, raw, time.monotonic() - started


def verify(name, url):
    print(f"\n== {name}: GET {url}")
    try:
        status, headers, raw, elapsed = fetch(url)
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        print(f"request failed: {error!r}")
        return False
    print(f"status {status} in {elapsed:.2f}s; body {len(raw)} bytes")
    lowered = {k.lower(): v for k, v in headers.items()}
    for key in SHOWN_HEADERS:
        if key in lowered:
            print(f"  header {key}: {lowered[key]}")
    if len(raw) > MAX_BODY_BYTES:
        print(f"body exceeds {MAX_BODY_BYTES} bytes; not parsed")
        return False
    if status != 200:
        print(f"non-200 body (first 200 bytes): {raw[:200]!r}")
        return False
    try:
        lines = summarise(name.split(" ")[0], json.loads(raw), int(time.time()))
    except (ValueError, KeyError, TypeError) as error:
        print(f"body did not match the expected shape: {error!r}")
        return False
    for line in lines:
        print(f"  {line}")
    return True


def self_test():
    window = {"data": {"2": {"avgHighPrice": None, "highPriceVolume": 0, "avgLowPrice": 227.22, "lowPriceVolume": 3},
                       "3": {"avgHighPrice": None, "highPriceVolume": 4, "avgLowPrice": 1, "lowPriceVolume": 1}},
              "timestamp": 1_795_504_200}
    lines = summarise("5m", window, 0)
    assert any("null average but non-zero volume: 1" in line for line in lines), lines
    assert any("multiple of 300: True" in line for line in lines), lines
    series = {"data": [{"timestamp": 0, "avgHighPrice": 1, "avgLowPrice": None, "highPriceVolume": 1, "lowPriceVolume": 0},
                       {"timestamp": 300, "avgHighPrice": 1, "avgLowPrice": None, "highPriceVolume": 1, "lowPriceVolume": 0}],
              "itemId": 1, "timestep": 300}
    assert any("{300: 1}" in line for line in summarise("timeseries", series, 0))
    latest = {"data": {"1": {"high": 5, "highTime": 100, "low": None, "lowTime": None}}}
    assert any("highTime age: 0" in line for line in summarise("latest", latest, 100))
    print("self-test passed")


def main(argv):
    if "--self-test" in argv:
        self_test()
        return 0
    plan = [
        ("mapping", "osrs-hc/mapping"),
        ("latest", f"osrs-hc/latest?id={SAMPLE_ITEM}"),
        ("5m", "osrs-hc/5m"),
        ("1h", "osrs-hc/1h"),
        ("timeseries", f"osrs-hc/timeseries?id={SAMPLE_ITEM}&lookback=24h"),
    ]
    if "--compare-standard" in argv:
        plan.append(("latest (standard world, comparison only)", f"osrs/latest?id={SAMPLE_ITEM}"))
    print(f"User-Agent: {USER_AGENT}\n{len(plan)} sequential requests, {PAUSE_SECONDS}s apart.")
    results = []
    for index, (name, path) in enumerate(plan):
        if index:
            time.sleep(PAUSE_SECONDS)
        results.append(verify(name, BASE + path))
    print(f"\n{sum(results)}/{len(results)} requests verified. Paste this output into the PR; it contains no bulk data.")
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
