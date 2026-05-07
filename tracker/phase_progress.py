#!/usr/bin/env python3
"""Phase progress dashboard: track MVL's Nine Phases roadmap."""

import marimo

__generated_with = "0.23.4"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import polars as pl
    import altair as alt
    import subprocess
    from pathlib import Path

    return alt, mo, pl, subprocess


@app.cell
def _(mo):
    mo.md("""
    # MVL Phase Progress Dashboard

    Tracking progress through the **Nine Phases** roadmap (spec 012).

    Data sourced from GitHub issue labels and milestone status.
    """)
    return


@app.cell
def _(pl):
    # Phase definitions from spec 012
    phases = pl.DataFrame({
        "phase": [1, 2, 3, 4, 5, 6, 7, 8, 9],
        "name": [
            "Foundation",
            "LLVM Backend",
            "Self-Hosting",
            "Bootstrap",
            "Backend Unification",
            "Maturity",
            "Verification",
            "Certification",
            "Production",
        ],
        "status": [
            "complete",
            "complete",
            "complete",
            "complete",
            "complete",
            "active",      # Current: stdlib, testing, examples
            "planned",     # Profiles, SMT
            "planned",
            "planned",
        ],
        "description": [
            "Parser, checker, Rust transpiler",
            "LLVM IR generation, mvl_memory",
            "Compiler compiles itself",
            "Native MVL compiler",
            "Cross-backend parity tests",
            "Stdlib complete, testing matures",
            "Stdlib profiles, proven mode",
            "Safety-critical certification",
            "Production-ready release",
        ],
    })

    phases
    return (phases,)


@app.cell
def _(pl, subprocess):
    def get_github_issues_by_label(label: str) -> int:
        """Count open issues with given label."""
        try:
            result = subprocess.run(
                ["gh", "issue", "list", "-l", label, "--state", "open", "--json", "number"],
                capture_output=True,
                text=True,
                cwd="/Users/iheitlager/wc/mvl_language",
            )
            import json
            issues = json.loads(result.stdout) if result.stdout else []
            return len(issues)
        except Exception:
            return 0

    # Get issue counts per phase
    phase_issues = pl.DataFrame({
        "phase": [5, 6, 7, 8, 9],
        "label": ["phase-5", "phase-6", "phase-7", "phase-8", "phase-9"],
    }).with_columns([
        pl.col("label").map_elements(get_github_issues_by_label, return_dtype=pl.Int64).alias("open_issues")
    ])

    phase_issues
    return (phase_issues,)


@app.cell
def _(alt, phase_issues, phases, pl):
    # Join phases with issue counts
    progress = phases.join(phase_issues, on="phase", how="left").with_columns([
        pl.col("open_issues").fill_null(0)
    ])

    # Status color mapping
    status_colors = {
        "complete": "#22c55e",
        "active": "#3b82f6",
        "planned": "#94a3b8",
    }

    chart = alt.Chart(progress.to_pandas()).mark_bar().encode(
        y=alt.Y("name:N", sort=None, title="Phase"),
        x=alt.X("open_issues:Q", title="Open Issues"),
        color=alt.Color("status:N", scale=alt.Scale(
            domain=["complete", "active", "planned"],
            range=["#22c55e", "#3b82f6", "#94a3b8"]
        )),
        tooltip=["phase", "name", "status", "open_issues", "description"],
    ).properties(
        width=500,
        height=350,
        title="Open Issues by Phase"
    )

    chart
    return (progress,)


@app.cell
def _(mo, pl, progress):
    # Current phase detail
    active = progress.filter(pl.col("status") == "active").row(0, named=True)

    mo.md(
        f"""
        ## Current Focus: Phase {active['phase']} — {active['name']}

        {active['description']}

        **Open issues:** {active['open_issues']}
        """
    )
    return


if __name__ == "__main__":
    app.run()
