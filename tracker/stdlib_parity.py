#!/usr/bin/env python3
"""Stdlib parity dashboard: compare Rust transpiler vs LLVM backend coverage."""

import marimo

__generated_with = "0.8.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import polars as pl
    import altair as alt
    import subprocess
    import json
    from pathlib import Path
    return mo, pl, alt, subprocess, json, Path


@app.cell
def _(mo):
    mo.md(
        """
        # MVL Stdlib Parity Dashboard

        Comparing stdlib function coverage between the **Rust transpiler** and **LLVM backend**.

        Target: both backends should support the same stdlib functions with identical behavior.
        """
    )
    return


@app.cell
def _(Path, subprocess):
    # Find repo root
    repo_root = Path(__file__).parent.parent

    def grep_stdlib_functions(pattern: str, path: str) -> list[str]:
        """Extract function names matching pattern from codebase."""
        try:
            result = subprocess.run(
                ["rg", "-o", pattern, str(repo_root / path)],
                capture_output=True,
                text=True,
            )
            return sorted(set(result.stdout.strip().split("\n"))) if result.stdout.strip() else []
        except Exception:
            return []

    return repo_root, grep_stdlib_functions


@app.cell
def _(grep_stdlib_functions, pl):
    # Extract stdlib functions from both backends
    # These patterns need tuning based on actual codebase structure

    # Rust transpiler: functions in mvl_runtime prelude
    rust_funcs = grep_stdlib_functions(
        r'pub fn (\w+)',
        "mvl_runtime/src"
    )

    # LLVM backend: builtin function dispatch
    llvm_funcs = grep_stdlib_functions(
        r'"mvl_(\w+)"',
        "src/mvl/codegen"
    )

    # Create comparison dataframe
    all_funcs = sorted(set(rust_funcs) | set(llvm_funcs))

    parity_df = pl.DataFrame({
        "function": all_funcs,
        "rust": [f in rust_funcs for f in all_funcs],
        "llvm": [f in llvm_funcs for f in all_funcs],
    }).with_columns([
        (pl.col("rust") & pl.col("llvm")).alias("both"),
        (pl.col("rust") & ~pl.col("llvm")).alias("rust_only"),
        (~pl.col("rust") & pl.col("llvm")).alias("llvm_only"),
    ])

    parity_df
    return rust_funcs, llvm_funcs, all_funcs, parity_df


@app.cell
def _(parity_df, mo):
    # Summary stats
    total = len(parity_df)
    both = parity_df.filter(pl.col("both")).height
    rust_only = parity_df.filter(pl.col("rust_only")).height
    llvm_only = parity_df.filter(pl.col("llvm_only")).height

    mo.md(
        f"""
        ## Summary

        | Metric | Count |
        |--------|-------|
        | Total functions | {total} |
        | Both backends | {both} |
        | Rust only | {rust_only} |
        | LLVM only | {llvm_only} |
        | **Parity %** | **{100 * both / total:.1f}%** |
        """
    )
    return total, both, rust_only, llvm_only


@app.cell
def _(parity_df, alt, pl):
    # Visualization: stacked bar showing parity status
    viz_df = pl.DataFrame({
        "status": ["Both", "Rust only", "LLVM only"],
        "count": [
            parity_df.filter(pl.col("both")).height,
            parity_df.filter(pl.col("rust_only")).height,
            parity_df.filter(pl.col("llvm_only")).height,
        ],
        "color": ["#22c55e", "#3b82f6", "#f97316"],
    })

    chart = alt.Chart(viz_df.to_pandas()).mark_bar().encode(
        x=alt.X("status:N", title="Backend Coverage"),
        y=alt.Y("count:Q", title="Function Count"),
        color=alt.Color("status:N", scale=alt.Scale(
            domain=["Both", "Rust only", "LLVM only"],
            range=["#22c55e", "#3b82f6", "#f97316"]
        )),
    ).properties(
        width=400,
        height=300,
        title="Stdlib Function Coverage by Backend"
    )

    chart
    return viz_df, chart


@app.cell
def _(parity_df, mo, pl):
    # Gap list: functions missing from one backend
    gaps = parity_df.filter(~pl.col("both")).select(["function", "rust", "llvm"])

    mo.md(
        f"""
        ## Gaps to Close

        Functions not yet implemented in both backends:
        """
    )
    return gaps,


@app.cell
def _(gaps):
    gaps
    return


if __name__ == "__main__":
    app.run()
