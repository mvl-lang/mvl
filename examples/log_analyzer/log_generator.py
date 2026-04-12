#!/usr/bin/env python3
"""
log_generator.py — Generate a sample JSON log file for the MVL log_analyzer.

Usage:
  python3 log_generator.py                    # writes logs.jsonl (200 entries)
  python3 log_generator.py -n 500             # 500 entries
  python3 log_generator.py -o /tmp/app.jsonl  # custom output path
  python3 log_generator.py --seed 42          # reproducible output

Each line is a valid JSON object:
  {"level": "info", "message": "...", "timestamp": 1700000000}

Feed to the analyzer:
  mvl run examples/log_analyzer/main.mvl -- --file logs.jsonl
  mvl run examples/log_analyzer/main.mvl -- --file logs.jsonl --level error
"""

import argparse
import json
import random
import time
from pathlib import Path

LEVELS = ["debug", "info", "warn", "error"]

LEVEL_WEIGHTS = [0.30, 0.45, 0.15, 0.10]  # realistic distribution

MESSAGES = {
    "debug": [
        "cache hit for key {key}",
        "query executed in {ms}ms",
        "connection pool size: {n}",
        "config reloaded",
        "GC cycle completed, freed {mb}MB",
    ],
    "info": [
        "server started on port {port}",
        "user {user_id} logged in",
        "request {method} {path} completed in {ms}ms",
        "scheduled job '{job}' finished",
        "deployment {version} activated",
        "health check passed",
    ],
    "warn": [
        "high memory usage: {pct}%",
        "slow query detected: {ms}ms",
        "retry {n}/3 for service {svc}",
        "disk usage at {pct}%",
        "deprecated API endpoint called: {path}",
    ],
    "error": [
        "database connection failed: {reason}",
        "unhandled exception in worker {worker_id}",
        "failed to write to {path}: permission denied",
        "service {svc} unavailable after {n} retries",
        "authentication failure for user {user_id}",
    ],
}

SUBSTITUTIONS = {
    "{key}": lambda: f"user:{random.randint(1000, 9999)}",
    "{ms}": lambda: str(random.randint(1, 2000)),
    "{n}": lambda: str(random.randint(1, 20)),
    "{mb}": lambda: str(random.randint(10, 512)),
    "{port}": lambda: str(random.choice([8080, 8443, 3000, 5000])),
    "{user_id}": lambda: str(random.randint(1, 9999)),
    "{method}": lambda: random.choice(["GET", "POST", "PUT", "DELETE"]),
    "{path}": lambda: random.choice(["/api/v1/users", "/api/v1/items", "/health", "/metrics"]),
    "{job}": lambda: random.choice(["cleanup", "report", "sync", "backup"]),
    "{version}": lambda: f"v{random.randint(1,5)}.{random.randint(0,9)}.{random.randint(0,9)}",
    "{pct}": lambda: str(random.randint(70, 99)),
    "{svc}": lambda: random.choice(["auth-service", "db-proxy", "cache", "mailer"]),
    "{reason}": lambda: random.choice(["timeout", "refused", "TLS error"]),
    "{worker_id}": lambda: str(random.randint(0, 7)),
}


def render(template: str) -> str:
    for placeholder, fn in SUBSTITUTIONS.items():
        if placeholder in template:
            template = template.replace(placeholder, fn(), 1)
    return template


def generate_entries(count: int, base_ts: int) -> list[dict]:
    entries = []
    ts = base_ts
    for _ in range(count):
        level = random.choices(LEVELS, weights=LEVEL_WEIGHTS, k=1)[0]
        template = random.choice(MESSAGES[level])
        message = render(template)
        ts += random.randint(1, 30)  # advance clock 1-30 seconds
        entries.append({"level": level, "message": message, "timestamp": ts})
    return entries


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate a JSON log file for the MVL log_analyzer example."
    )
    parser.add_argument("-n", "--count", type=int, default=200, help="Number of log entries (default: 200)")
    parser.add_argument("-o", "--output", type=Path, default=Path("logs.jsonl"), help="Output file (default: logs.jsonl)")
    parser.add_argument("--seed", type=int, default=None, help="Random seed for reproducible output")
    args = parser.parse_args()

    if args.seed is not None:
        random.seed(args.seed)

    base_ts = int(time.time()) - args.count * 30  # start ~count*30s ago
    entries = generate_entries(args.count, base_ts)

    with open(args.output, "w") as f:
        for entry in entries:
            f.write(json.dumps(entry) + "\n")

    # Print summary to stderr so it doesn't pollute piped output
    import sys
    levels_seen = {}
    for e in entries:
        levels_seen[e["level"]] = levels_seen.get(e["level"], 0) + 1

    print(f"Generated {len(entries)} entries → {args.output}", file=sys.stderr)
    for level in LEVELS:
        print(f"  {level:5s}: {levels_seen.get(level, 0)}", file=sys.stderr)


if __name__ == "__main__":
    main()
