#!/usr/bin/env python3
"""Release Timeline — version history and changelog entries."""

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
    from datetime import datetime

    return Path, alt, datetime, mo, pl, re, subprocess


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
    # Release Timeline

    Version history, release velocity, and changelog highlights.

    **Repo:** `{repo_display}`
    """)
    return


@app.cell
def _(re, repo_root, subprocess):
    # Get git tags with dates
    def get_releases() -> list[dict]:
        """Extract version tags with commit dates."""
        try:
            # Get tags sorted by version
            result = subprocess.run(
                ["git", "tag", "-l", "v*", "--sort=-v:refname"],
                capture_output=True,
                text=True,
                cwd=str(repo_root),
            )
            tags = result.stdout.strip().split("\n")

            releases = []
            for tag in tags[:30]:  # Last 30 releases
                if not tag:
                    continue
                # Get tag date
                date_result = subprocess.run(
                    ["git", "log", "-1", "--format=%ci", tag],
                    capture_output=True,
                    text=True,
                    cwd=str(repo_root),
                )
                date_str = date_result.stdout.strip()[:10]  # YYYY-MM-DD

                # Parse version
                match = re.match(r"v(\d+)\.(\d+)\.(\d+)", tag)
                if match:
                    releases.append({
                        "tag": tag,
                        "major": int(match.group(1)),
                        "minor": int(match.group(2)),
                        "patch": int(match.group(3)),
                        "date": date_str,
                    })
            return releases
        except Exception:
            return []

    releases = get_releases()
    return get_releases, releases


@app.cell
def _(mo, pl, releases):
    if releases:
        release_df = pl.DataFrame(releases)

        # Current version
        current = releases[0] if releases else {"tag": "unknown"}

        mo.md(f"""
        ## Current Version: `{current['tag']}`

        **Total releases:** {len(releases)}
        """)
    else:
        release_df = pl.DataFrame()
        mo.md("*No releases found*")
    return current, release_df


@app.cell
def _(mo):
    mo.md("## Recent Releases")
    return


@app.cell
def _(pl, release_df):
    if len(release_df) > 0:
        release_df.head(15)
    return


@app.cell
def _(alt, pl, releases):
    # Release velocity chart (releases per month)
    if releases:
        # Group by month
        monthly = {}
        for r in releases:
            month = r["date"][:7]  # YYYY-MM
            monthly[month] = monthly.get(month, 0) + 1

        velocity_data = [{"month": k, "releases": v} for k, v in sorted(monthly.items())[-12:]]
        velocity_df = pl.DataFrame(velocity_data)

        chart = alt.Chart(velocity_df.to_pandas()).mark_bar().encode(
            x=alt.X("month:N", title="Month", sort=None),
            y=alt.Y("releases:Q", title="Releases"),
            tooltip=["month", "releases"],
        ).properties(
            width=500,
            height=200,
            title="Release Velocity (last 12 months)"
        )

        chart
    return chart, monthly, velocity_data, velocity_df


@app.cell
def _(mo, repo_root):
    # Parse CHANGELOG.md for recent entries
    changelog_path = repo_root / "CHANGELOG.md"

    if changelog_path.exists():
        content = changelog_path.read_text()
        # Get first 2000 chars (recent entries)
        preview = content[:2000]
        if len(content) > 2000:
            preview += "\n\n*... (truncated)*"

        mo.md(f"""
        ## Changelog Preview

        ```markdown
        {preview}
        ```
        """)
    else:
        mo.md("*CHANGELOG.md not found*")
    return changelog_path, content, preview


if __name__ == "__main__":
    app.run()
