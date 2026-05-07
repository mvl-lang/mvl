#!/usr/bin/env python3
"""Epic Tracker — monitor major epics and sub-ticket progress."""

import marimo

__generated_with = "0.10.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import polars as pl
    import altair as alt
    import subprocess
    import json
    from pathlib import Path

    return Path, alt, json, mo, pl, subprocess


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
    # Epic Tracker

    Track major epics and their sub-ticket progress.

    **Repo:** `{repo_display}`
    """)
    return


@app.cell
def _(json, subprocess):
    # Key epics to track
    EPICS = [
        {"number": 533, "name": "Stdlib Profiles", "description": "Trusted vs proven stdlib modes"},
        {"number": 314, "name": "Stdlib Stubs", "description": "Replace stubs with real implementations"},
    ]

    def get_issue_details(issue_num: int) -> dict:
        """Fetch issue details from GitHub."""
        try:
            result = subprocess.run(
                ["gh", "issue", "view", str(issue_num), "--json", "title,state,body,labels"],
                capture_output=True,
                text=True,
                cwd="/Users/iheitlager/wc/mvl_language",
                timeout=10,
            )
            if result.returncode == 0:
                return json.loads(result.stdout)
        except Exception:
            pass
        return {}

    def get_sub_issues(epic_num: int) -> list[dict]:
        """Find issues that reference an epic."""
        try:
            # Search for issues mentioning the epic
            result = subprocess.run(
                ["gh", "issue", "list", "--search", f"#{epic_num}", "--json", "number,title,state,labels", "--limit", "50"],
                capture_output=True,
                text=True,
                cwd="/Users/iheitlager/wc/mvl_language",
                timeout=15,
            )
            if result.returncode == 0:
                issues = json.loads(result.stdout)
                # Filter out the epic itself
                return [i for i in issues if i["number"] != epic_num]
        except Exception:
            pass
        return []

    return EPICS, get_issue_details, get_sub_issues


@app.cell
def _(EPICS, get_issue_details, get_sub_issues, mo, pl):
    # Build epic summary
    epic_data = []
    for epic in EPICS:
        details = get_issue_details(epic["number"])
        sub_issues = get_sub_issues(epic["number"])

        open_count = sum(1 for i in sub_issues if i.get("state") == "OPEN")
        closed_count = sum(1 for i in sub_issues if i.get("state") == "CLOSED")
        total = open_count + closed_count

        epic_data.append({
            "epic": f"#{epic['number']}",
            "name": epic["name"],
            "description": epic["description"],
            "state": details.get("state", "UNKNOWN"),
            "open": open_count,
            "closed": closed_count,
            "total": total,
            "progress": f"{100 * closed_count / total:.0f}%" if total > 0 else "N/A",
        })

    epic_df = pl.DataFrame(epic_data)

    mo.md("## Epic Summary")
    return epic_data, epic_df


@app.cell
def _(epic_df):
    epic_df
    return


@app.cell
def _(alt, epic_df):
    # Progress chart
    chart_data = epic_df.select(["name", "open", "closed"]).to_pandas()
    chart_data = chart_data.melt(id_vars=["name"], var_name="status", value_name="count")

    chart = alt.Chart(chart_data).mark_bar().encode(
        x=alt.X("name:N", title="Epic"),
        y=alt.Y("count:Q", title="Issues"),
        color=alt.Color("status:N", scale=alt.Scale(
            domain=["closed", "open"],
            range=["#22c55e", "#f97316"]
        )),
        tooltip=["name", "status", "count"],
    ).properties(
        width=400,
        height=250,
        title="Epic Progress"
    )

    chart
    return chart, chart_data


@app.cell
def _(EPICS, get_sub_issues, mo, pl):
    # Detailed sub-issue list for first epic
    mo.md("## Sub-Issues Detail")

    all_subs = []
    for epic in EPICS:
        subs = get_sub_issues(epic["number"])
        for s in subs:
            labels = [l.get("name", "") for l in s.get("labels", [])]
            all_subs.append({
                "epic": f"#{epic['number']}",
                "issue": f"#{s['number']}",
                "title": s.get("title", "")[:60],
                "state": s.get("state", ""),
                "labels": ", ".join(labels[:3]),
            })

    if all_subs:
        sub_df = pl.DataFrame(all_subs)
        sub_df
    else:
        mo.md("*No sub-issues found*")
    return all_subs, sub_df


if __name__ == "__main__":
    app.run()
