#!/usr/bin/env python3
"""Codebase Metrics — lines of code, module sizes, growth over time."""

import marimo

__generated_with = "0.10.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import polars as pl
    import altair as alt
    import subprocess
    from pathlib import Path

    return Path, alt, mo, pl, subprocess


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
    # Codebase Metrics

    Lines of code, module sizes, and growth tracking.

    **Repo:** `{repo_display}`
    """)
    return


@app.cell
def _(repo_root, subprocess):
    # Count lines using tokei or wc
    def count_lines_by_dir(path: str) -> dict:
        """Count Rust lines of code in a directory."""
        full_path = repo_root / path
        if not full_path.exists():
            return {"path": path, "files": 0, "lines": 0}

        try:
            # Use find + wc for portability
            result = subprocess.run(
                f'find "{full_path}" -name "*.rs" -type f | xargs wc -l 2>/dev/null | tail -1',
                shell=True,
                capture_output=True,
                text=True,
            )
            # Parse "  12345 total" or just "  12345 filename"
            output = result.stdout.strip()
            if output:
                parts = output.split()
                lines = int(parts[0]) if parts else 0
            else:
                lines = 0

            # Count files
            file_result = subprocess.run(
                f'find "{full_path}" -name "*.rs" -type f | wc -l',
                shell=True,
                capture_output=True,
                text=True,
            )
            files = int(file_result.stdout.strip()) if file_result.stdout.strip() else 0

            return {"path": path, "files": files, "lines": lines}
        except Exception:
            return {"path": path, "files": 0, "lines": 0}

    # Key directories
    directories = [
        "src/mvl/parser",
        "src/mvl/resolver",
        "src/mvl/checker",
        "src/mvl/transpiler",
        "src/mvl/codegen",
        "src/mvl/passes",
        "mvl_runtime/src",
        "mvl_memory/src",
        "tests",
    ]

    metrics = [count_lines_by_dir(d) for d in directories]
    return count_lines_by_dir, directories, metrics


@app.cell
def _(metrics, mo, pl):
    metrics_df = pl.DataFrame(metrics)

    total_lines = metrics_df.select(pl.sum("lines")).item()
    total_files = metrics_df.select(pl.sum("files")).item()

    mo.md(f"""
    ## Summary

    | Metric | Value |
    |--------|-------|
    | Total Rust files | {total_files} |
    | Total lines of code | {total_lines:,} |
    """)
    return metrics_df, total_files, total_lines


@app.cell
def _(mo):
    mo.md("## Lines by Module")
    return


@app.cell
def _(metrics_df):
    metrics_df.sort("lines", descending=True)
    return


@app.cell
def _(alt, metrics_df):
    # Bar chart of lines by module
    chart = alt.Chart(metrics_df.to_pandas()).mark_bar().encode(
        y=alt.Y("path:N", title="Module", sort="-x"),
        x=alt.X("lines:Q", title="Lines of Code"),
        color=alt.value("#3b82f6"),
        tooltip=["path", "files", "lines"],
    ).properties(
        width=450,
        height=300,
        title="Lines of Code by Module"
    )

    chart
    return (chart,)


@app.cell
def _(repo_root, subprocess):
    # Git commit count for growth proxy
    def get_commit_count() -> int:
        try:
            result = subprocess.run(
                ["git", "rev-list", "--count", "HEAD"],
                capture_output=True,
                text=True,
                cwd=str(repo_root),
            )
            return int(result.stdout.strip())
        except Exception:
            return 0

    commit_count = get_commit_count()
    return commit_count, get_commit_count


@app.cell
def _(commit_count, mo, total_lines):
    # Growth stats
    lines_per_commit = total_lines / commit_count if commit_count > 0 else 0

    mo.md(f"""
    ## Growth Metrics

    | Metric | Value |
    |--------|-------|
    | Total commits | {commit_count} |
    | Lines per commit | {lines_per_commit:.1f} |
    """)
    return (lines_per_commit,)


@app.cell
def _(repo_root, subprocess):
    # Recent contributors
    def get_recent_contributors(days: int = 30) -> list[str]:
        try:
            result = subprocess.run(
                ["git", "log", f"--since={days} days ago", "--format=%an", "--no-merges"],
                capture_output=True,
                text=True,
                cwd=str(repo_root),
            )
            names = result.stdout.strip().split("\n")
            # Count unique
            from collections import Counter
            counts = Counter(names)
            return [f"{name} ({count})" for name, count in counts.most_common(10)]
        except Exception:
            return []

    contributors = get_recent_contributors()
    return contributors, get_recent_contributors


@app.cell
def _(contributors, mo):
    if contributors:
        contrib_list = "\n".join(f"- {c}" for c in contributors)
        mo.md(f"""
        ## Recent Contributors (30 days)

        {contrib_list}
        """)
    return (contrib_list,)


if __name__ == "__main__":
    app.run()
