#!/usr/bin/env python3
"""Test Results — cross-backend test pass/fail and coverage."""

import marimo

__generated_with = "0.10.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import polars as pl
    import altair as alt
    import subprocess
    import re
    from pathlib import Path

    return Path, alt, mo, pl, re, subprocess


@app.cell
def _(Path):
    repo_root = Path(__file__).parent.parent.resolve()

    def shorten_path(p: Path | str) -> str:
        s = str(p)
        home = str(Path.home())
        return s.replace(home, "~") if s.startswith(home) else s

    repo_display = shorten_path(repo_root)
    return repo_display, repo_root, shorten_path


@app.cell
def _(mo, repo_display):
    mo.md(f"""
    # Test Results Dashboard

    Cross-backend test results and coverage metrics.

    **Repo:** `{repo_display}`
    """)
    return


@app.cell
def _(mo):
    refresh_btn = mo.ui.button(label="🔄 Run Tests", kind="success")
    refresh_btn
    return (refresh_btn,)


@app.cell
def _(re, refresh_btn, repo_root, subprocess):
    # Run cargo test and parse results
    refresh_btn  # Dependency to trigger on button click

    def run_tests(test_filter: str = "") -> dict:
        """Run cargo test and parse output."""
        cmd = ["cargo", "test", "--no-fail-fast"]
        if test_filter:
            cmd.extend(["--", test_filter])

        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                cwd=str(repo_root),
                timeout=120,
            )
            output = result.stdout + result.stderr

            # Parse test result line: "test result: ok. X passed; Y failed; Z ignored"
            match = re.search(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", output)
            if match:
                return {
                    "passed": int(match.group(1)),
                    "failed": int(match.group(2)),
                    "ignored": int(match.group(3)),
                    "success": result.returncode == 0,
                }
        except subprocess.TimeoutExpired:
            return {"passed": 0, "failed": 0, "ignored": 0, "success": False, "error": "timeout"}
        except Exception as e:
            return {"passed": 0, "failed": 0, "ignored": 0, "success": False, "error": str(e)}

        return {"passed": 0, "failed": 0, "ignored": 0, "success": False}

    # Run different test suites
    test_suites = {
        "unit": run_tests("--lib"),
        "compile_and_run": run_tests("--test compile_and_run"),
        "cross_backend": run_tests("--test cross_backend"),
    }

    return run_tests, test_suites


@app.cell
def _(mo, pl, test_suites):
    # Summary table
    rows = []
    for suite, results in test_suites.items():
        total = results["passed"] + results["failed"]
        rate = f"{100 * results['passed'] / total:.1f}%" if total > 0 else "N/A"
        rows.append({
            "suite": suite,
            "passed": results["passed"],
            "failed": results["failed"],
            "ignored": results["ignored"],
            "pass_rate": rate,
            "status": "✅" if results.get("success") else "❌",
        })

    results_df = pl.DataFrame(rows)

    mo.md("## Test Suite Summary")
    return results_df, rows


@app.cell
def _(results_df):
    results_df
    return


@app.cell
def _(alt, pl, test_suites):
    # Visualization
    chart_data = []
    for suite, results in test_suites.items():
        chart_data.append({"suite": suite, "outcome": "passed", "count": results["passed"]})
        chart_data.append({"suite": suite, "outcome": "failed", "count": results["failed"]})

    chart_df = pl.DataFrame(chart_data)

    chart = alt.Chart(chart_df.to_pandas()).mark_bar().encode(
        x=alt.X("suite:N", title="Test Suite"),
        y=alt.Y("count:Q", title="Tests"),
        color=alt.Color("outcome:N", scale=alt.Scale(
            domain=["passed", "failed"],
            range=["#22c55e", "#ef4444"]
        )),
        tooltip=["suite", "outcome", "count"],
    ).properties(
        width=400,
        height=250,
        title="Test Results by Suite"
    )

    chart
    return chart, chart_data, chart_df


@app.cell
def _(mo, test_suites):
    # Overall status
    total_passed = sum(r["passed"] for r in test_suites.values())
    total_failed = sum(r["failed"] for r in test_suites.values())
    total = total_passed + total_failed

    if total_failed == 0:
        status = "🟢 All tests passing"
    elif total_failed < 5:
        status = f"🟡 {total_failed} tests failing"
    else:
        status = f"🔴 {total_failed} tests failing"

    mo.md(f"""
    ## Overall Status

    **{status}**

    - Total: {total} tests
    - Passed: {total_passed}
    - Failed: {total_failed}
    """)
    return status, total, total_failed, total_passed


if __name__ == "__main__":
    app.run()
