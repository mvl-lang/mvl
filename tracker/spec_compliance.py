#!/usr/bin/env python3
"""Spec Compliance — requirements vs implementation status."""

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
    specs_dir = repo_root / ".openspec" / "specs"

    def shorten_path(p: Path | str) -> str:
        s = str(p)
        home = str(Path.home())
        return s.replace(home, "~") if s.startswith(home) else s

    repo_display = shorten_path(repo_root)
    return repo_display, repo_root, shorten_path, specs_dir


@app.cell
def _(mo, repo_display):
    mo.md(f"""
    # Spec Compliance Dashboard

    Track requirements from `.openspec/specs/` against implementation status.

    **Repo:** `{repo_display}`
    """)
    return


@app.cell
def _(re, specs_dir):
    # Parse specs for requirements
    def parse_spec(spec_path) -> list[dict]:
        """Extract requirements from a spec file."""
        requirements = []
        try:
            content = spec_path.read_text()

            # Match requirements like: ### Requirement N: Title [MUST|SHOULD|MAY]
            pattern = r"###\s+Requirement\s+(\d+):\s+(.+?)\s+\[(MUST|SHOULD|MAY)\]"
            matches = re.findall(pattern, content)

            for num, title, level in matches:
                # Check for Implementation: tag
                impl_pattern = rf"Requirement\s+{num}.*?\*\*Implementation:\*\*\s+`([^`]+)`"
                impl_match = re.search(impl_pattern, content, re.DOTALL)
                impl = impl_match.group(1) if impl_match else None

                # Check for Tests: tag
                test_pattern = rf"Requirement\s+{num}.*?\*\*Tests:\*\*\s+`([^`]+)`"
                test_match = re.search(test_pattern, content, re.DOTALL)
                tests = test_match.group(1) if test_match else None

                requirements.append({
                    "spec": spec_path.parent.name,
                    "req_num": int(num),
                    "title": title.strip(),
                    "level": level,
                    "implemented": impl is not None,
                    "tested": tests is not None,
                    "impl_path": impl,
                    "test_path": tests,
                })
        except Exception:
            pass
        return requirements

    # Find all specs
    all_requirements = []
    if specs_dir.exists():
        for spec_dir in sorted(specs_dir.iterdir()):
            spec_file = spec_dir / "spec.md"
            if spec_file.exists():
                all_requirements.extend(parse_spec(spec_file))

    return all_requirements, parse_spec


@app.cell
def _(all_requirements, mo, pl):
    if all_requirements:
        req_df = pl.DataFrame(all_requirements)

        total = len(req_df)
        implemented = req_df.filter(pl.col("implemented")).height
        tested = req_df.filter(pl.col("tested")).height

        mo.md(f"""
        ## Summary

        | Metric | Count | Percentage |
        |--------|-------|------------|
        | Total Requirements | {total} | 100% |
        | Implemented | {implemented} | {100*implemented/total:.0f}% |
        | Tested | {tested} | {100*tested/total:.0f}% |
        """)
    else:
        req_df = pl.DataFrame()
        mo.md("*No specs found in `.openspec/specs/`*")
    return implemented, req_df, tested, total


@app.cell
def _(mo):
    mo.md("## Requirements by Spec")
    return


@app.cell
def _(all_requirements, alt, pl):
    if all_requirements:
        # Group by spec
        spec_summary = {}
        for r in all_requirements:
            spec = r["spec"]
            if spec not in spec_summary:
                spec_summary[spec] = {"total": 0, "implemented": 0, "tested": 0}
            spec_summary[spec]["total"] += 1
            if r["implemented"]:
                spec_summary[spec]["implemented"] += 1
            if r["tested"]:
                spec_summary[spec]["tested"] += 1

        chart_data = []
        for spec, counts in spec_summary.items():
            chart_data.append({"spec": spec, "status": "implemented", "count": counts["implemented"]})
            chart_data.append({"spec": spec, "status": "not implemented", "count": counts["total"] - counts["implemented"]})

        chart_df = pl.DataFrame(chart_data)

        chart = alt.Chart(chart_df.to_pandas()).mark_bar().encode(
            y=alt.Y("spec:N", title="Spec", sort=None),
            x=alt.X("count:Q", title="Requirements"),
            color=alt.Color("status:N", scale=alt.Scale(
                domain=["implemented", "not implemented"],
                range=["#22c55e", "#e5e7eb"]
            )),
            tooltip=["spec", "status", "count"],
        ).properties(
            width=400,
            height=250,
            title="Implementation Status by Spec"
        )

        chart
    return chart, chart_data, chart_df, spec_summary


@app.cell
def _(mo):
    mo.md("## Requirement Details")
    return


@app.cell
def _(pl, req_df):
    if len(req_df) > 0:
        req_df.select(["spec", "req_num", "title", "level", "implemented", "tested"])
    return


@app.cell
def _(mo, pl, req_df):
    # Gaps: requirements without implementation
    if len(req_df) > 0:
        gaps = req_df.filter(~pl.col("implemented"))
        if len(gaps) > 0:
            mo.md(f"""
            ## Implementation Gaps

            **{len(gaps)} requirements** without implementation links:
            """)
        else:
            mo.md("## ✅ All requirements have implementation links!")
    return (gaps,)


@app.cell
def _(gaps, pl):
    if len(gaps) > 0:
        gaps.select(["spec", "req_num", "title", "level"])
    return


if __name__ == "__main__":
    app.run()
