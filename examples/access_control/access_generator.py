#!/usr/bin/env python3
"""access_generator.py — Generate access attempt scenarios for manual testing.

Produces JSONL records describing (username, resource, action) access attempts,
useful for stress-testing the access_control demo or future CI regression suites.

Usage:
    python3 access_generator.py                    # 20 random scenarios to stdout
    python3 access_generator.py -o scenarios.jsonl # write to file
    python3 access_generator.py -n 50 --seed 42    # reproducible output
"""

import argparse
import json
import random
import sys


USERS = [
    {"username": "alice",   "role": "Admin"},
    {"username": "bob",     "role": "User"},
    {"username": "mod1",    "role": "Moderator"},
    {"username": "visitor", "role": "Guest"},
]

RESOURCES = ["UserProfile", "AdminPanel", "Post", "AuditLog"]
ACTIONS    = ["Read", "Write", "Delete"]

# Expected policy outcomes (mirrors check_permission in main.mvl)
POLICY = {
    ("Admin",     "*",            "*")        : "Allow",
    ("Moderator", "AdminPanel",   "*")        : "Deny",
    ("Moderator", "AuditLog",     "*")        : "Allow",
    ("Moderator", "UserProfile",  "Delete")   : "Deny",
    ("Moderator", "UserProfile",  "*")        : "Allow",
    ("Moderator", "Post",         "*")        : "Allow",
    ("User",      "AdminPanel",   "*")        : "Deny",
    ("User",      "AuditLog",     "*")        : "Deny",
    ("User",      "UserProfile",  "Delete")   : "Deny",
    ("User",      "UserProfile",  "*")        : "Allow",
    ("User",      "Post",         "Delete")   : "Deny",
    ("User",      "Post",         "*")        : "Allow",
    ("Guest",     "AdminPanel",   "*")        : "Deny",
    ("Guest",     "AuditLog",     "*")        : "Deny",
    ("Guest",     "UserProfile",  "Write")    : "Deny",
    ("Guest",     "UserProfile",  "Delete")   : "Deny",
    ("Guest",     "UserProfile",  "Read")     : "Allow",
    ("Guest",     "Post",         "Write")    : "Deny",
    ("Guest",     "Post",         "Delete")   : "Deny",
    ("Guest",     "Post",         "Read")     : "Allow",
}


def expected_decision(role: str, resource: str, action: str) -> str:
    for (r, res, act), decision in POLICY.items():
        if r == role or r == "*":
            if res == resource or res == "*":
                if act == action or act == "*":
                    return decision
    return "Deny"  # deny by default if not in policy


def generate_scenario(rng: random.Random) -> dict:
    user = rng.choice(USERS)
    resource = rng.choice(RESOURCES)
    action = rng.choice(ACTIONS)
    decision = expected_decision(user["role"], resource, action)
    return {
        "username": user["username"],
        "role": user["role"],
        "resource": resource,
        "action": action,
        "expected": decision,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("-n", "--count",  type=int, default=20, help="Number of scenarios (default: 20)")
    parser.add_argument("-o", "--output", type=str, default="-",  help="Output file (default: stdout)")
    parser.add_argument("--seed",         type=int, default=None,  help="Random seed for reproducibility")
    args = parser.parse_args()

    rng = random.Random(args.seed)
    scenarios = [generate_scenario(rng) for _ in range(args.count)]

    out = open(args.output, "w") if args.output != "-" else sys.stdout
    try:
        for s in scenarios:
            print(json.dumps(s), file=out)
    finally:
        if args.output != "-":
            out.close()

    if args.output != "-":
        print(f"Generated {args.count} scenarios → {args.output}", file=sys.stderr)


if __name__ == "__main__":
    main()
