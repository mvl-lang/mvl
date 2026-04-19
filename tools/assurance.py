#!/usr/bin/env python3
"""MVL Assurance Checker — validates ISPE traceability.

Scans .openspec/specs/ for requirements and checks:
1. Completeness (S→P): every requirement has an **Implementation:** link
2. Coverage (T→P): every requirement has a **Tests:** link
3. Corpus: every requirement with a **Corpus:** link has the file present
4. Scenarios: counts scenarios per requirement

Reports a dashboard and exits non-zero if below thresholds.

Usage:
    python3 tools/assurance.py              # dashboard
    python3 tools/assurance.py --verbose     # show each requirement
    python3 tools/assurance.py --min 0.75    # CI gate: exit 1 if below 75%
"""

import argparse
import re
import sys
from pathlib import Path

SPEC_DIR = Path(__file__).parent.parent / ".openspec" / "specs"
SRC_DIR = Path(__file__).parent.parent / "src"
TESTS_DIR = Path(__file__).parent.parent / "tests"


def parse_specs():
    """Parse all spec files and extract requirements."""
    requirements = []
    for spec_dir in sorted(SPEC_DIR.iterdir()):
        spec_file = spec_dir / "spec.md" if spec_dir.is_dir() else None
        if not spec_file or not spec_file.exists():
            continue

        text = spec_file.read_text()
        spec_name = spec_dir.name

        # Find all requirements
        req_blocks = re.split(r"(?=^### Requirement \d+)", text, flags=re.MULTILINE)
        for block in req_blocks:
            m = re.match(r"### Requirement (\d+): (.+?) \[(\w+)\]", block)
            if not m:
                continue

            num, title, level = m.group(1), m.group(2), m.group(3)

            # Check for Implementation link
            impl_match = re.search(r"\*\*Implementation:\*\*\s*`(.+?)`", block)
            impl_path = impl_match.group(1) if impl_match else None
            impl_file = impl_path.split("::")[0].strip() if impl_path else None
            if impl_file:
                _resolved = (SRC_DIR.parent / impl_file).resolve()
                _repo_root = SRC_DIR.parent.resolve()
                impl_exists = _resolved.is_relative_to(_repo_root) and _resolved.exists()
            else:
                impl_exists = False

            # Check for Tests link
            tests_match = re.search(r"\*\*Tests:\*\*\s*(.+)", block)
            tests_path = tests_match.group(1).strip() if tests_match else None

            # Check for Corpus link
            corpus_files = re.findall(r"\*\*Corpus:\*\*\s*`(.+?)`", block)
            corpus_present = all(
                (SRC_DIR.parent / f).exists() for f in corpus_files
            )

            # Count scenarios
            scenarios = len(re.findall(r"#### Scenario:", block))

            requirements.append(
                {
                    "spec": spec_name,
                    "num": int(num),
                    "title": title,
                    "level": level,
                    "impl_path": impl_path,
                    "impl_exists": impl_exists,
                    "tests_path": tests_path,
                    "tests_linked": tests_path is not None,
                    "corpus_files": corpus_files,
                    "corpus_present": corpus_present,
                    "scenarios": scenarios,
                }
            )

    return requirements


def _get_test_coverage():
    """Try to get line coverage from cargo-tarpaulin or cargo-llvm-cov output.

    Returns a string like '87.3%' or None if no coverage tool is available.
    Doesn't run coverage itself — reads cached results if present.
    """
    import subprocess

    # Try llvm-cov cache (macOS + Linux)
    llvm_cov_out = Path(__file__).parent.parent / "target" / "llvm-cov.json"
    if llvm_cov_out.exists():
        try:
            import json
            data = json.loads(llvm_cov_out.read_text())
            lines = data["data"][0]["totals"]["lines"]
            return f"{lines['percent']:.1f}% ({lines['covered']}/{lines['count']} lines)"
        except (json.JSONDecodeError, KeyError, IndexError):
            pass

    # Try tarpaulin cache (Linux only)
    tarpaulin_out = Path(__file__).parent.parent / "target" / "tarpaulin" / "coverage.json"
    if tarpaulin_out.exists():
        try:
            import json
            data = json.loads(tarpaulin_out.read_text())
            if "coverage" in data:
                return f"{data['coverage']:.1f}%"
        except (json.JSONDecodeError, KeyError):
            pass

    # Try running cargo test to at least count tests
    try:
        result = subprocess.run(
            ["cargo", "test", "--", "--list"],
            capture_output=True, text=True, timeout=30,
            cwd=Path(__file__).parent.parent,
        )
        if result.returncode == 0:
            test_count = sum(1 for line in result.stdout.splitlines() if ": test" in line)
            if test_count > 0:
                return f"{test_count} tests (run `cargo tarpaulin` for line coverage)"
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass

    return None


def report(requirements, verbose=False):
    """Print assurance dashboard."""
    total = len(requirements)
    if total == 0:
        print("No requirements found in .openspec/specs/")
        return 0.0, 0.0, 1.0

    impl_linked = sum(1 for r in requirements if r["impl_path"])
    impl_exists = sum(1 for r in requirements if r["impl_exists"])
    tests_linked = sum(1 for r in requirements if r["tests_linked"])
    corpus_present = sum(
        1 for r in requirements if r["corpus_files"] and r["corpus_present"]
    )
    corpus_total = sum(1 for r in requirements if r["corpus_files"])
    total_scenarios = sum(r["scenarios"] for r in requirements)

    completeness = impl_exists / total if total else 0
    coverage = tests_linked / total if total else 0

    # Assurance = of the implemented requirements, how many have evidence (tests)?
    assured = sum(
        1 for r in requirements if r["impl_exists"] and r["tests_linked"]
    )
    assurance = assured / impl_exists if impl_exists else 1.0  # no impl = nothing to assure = 100%

    # Test coverage: run cargo test with coverage if available
    test_coverage = _get_test_coverage()

    print("=" * 60)
    print("MVL Assurance Dashboard (ISPE)")
    print("=" * 60)
    print(f"Requirements:     {total}")
    print(f"Scenarios:        {total_scenarios}")
    print()
    print(f"Completeness (S->P):  {impl_exists}/{total} spec -> implementation  ({completeness:.0%})")
    print(f"  - Linked:           {impl_linked}/{total}")
    print(f"  - File exists:      {impl_exists}/{total}")
    print()
    print(f"Coverage (E->P):      {tests_linked}/{total} evidence linked  ({coverage:.0%})")
    if test_coverage is not None:
        print(f"  - Line coverage:    {test_coverage}")
    print()
    print(f"Assurance (E/P):      {assured}/{impl_exists} of implemented have evidence  ({assurance:.0%})")
    print()
    if corpus_total:
        print(f"Corpus:               {corpus_present}/{corpus_total} present")
    print("=" * 60)

    if verbose:
        print()
        print("  Legend: [impl][tests][corpus]")
        print("    impl:   ✓=exists  ○=linked/missing  ✗=not linked")
        print("    tests:  T=linked  -=none")
        print("    corpus: C=present c=linked/missing  -=none")
        print()
        for r in requirements:
            status = "✓" if r["impl_exists"] else "○" if r["impl_path"] else "✗"
            test_status = "T" if r["tests_linked"] else "-"
            corpus_status = (
                "C"
                if r["corpus_files"] and r["corpus_present"]
                else "c"
                if r["corpus_files"]
                else "-"
            )
            print(
                f"  [{status}][{test_status}][{corpus_status}] "
                f"{r['spec']}/Req {r['num']}: {r['title']} "
                f"({r['scenarios']} scenarios)"
            )

    return completeness, coverage, assurance


def main():
    parser = argparse.ArgumentParser(description="MVL Assurance Checker")
    parser.add_argument("-v", "--verbose", action="store_true", help="Show each requirement")
    parser.add_argument("--min", type=float, default=0.0, help="Minimum assurance score (0.0-1.0) for CI gate")
    args = parser.parse_args()

    requirements = parse_specs()
    completeness, coverage, assurance = report(requirements, verbose=args.verbose)

    if args.min > 0:
        if assurance < args.min:
            print(f"\nFAIL: assurance {assurance:.0%} below threshold {args.min:.0%}")
            print(f"  completeness: {completeness:.0%}")
            print(f"  coverage:     {coverage:.0%}")
            print(f"  assurance:    {assurance:.0%}")
            sys.exit(1)
        else:
            print(f"\nPASS: assurance {assurance:.0%} above threshold {args.min:.0%}")


if __name__ == "__main__":
    main()
