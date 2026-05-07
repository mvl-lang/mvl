#!/usr/bin/env python3
"""MVL Tracker — Dashboard Launcher."""

import marimo

__generated_with = "0.10.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    return (mo,)


@app.cell
def _(mo):
    mo.md(
        """
        # MVL Tracker

        Compiler metrics and progress dashboards for the Minimum Verification Language.

        ---

        ## Dashboards

        | Dashboard | Description |
        |-----------|-------------|
        | [**Stdlib Parity**](/stdlib) | Compare stdlib coverage: Rust transpiler vs LLVM backend |
        | [**Phase Progress**](/phases) | Track the Nine Phases roadmap with GitHub issue counts |
        | [**Epic Tracker**](/epics) | Monitor major epics (#533, #314) and sub-ticket progress |
        | [**Test Results**](/tests) | Cross-backend test pass/fail and coverage metrics |
        | [**Release Timeline**](/releases) | Version history, changelog entries, release velocity |
        | [**Spec Compliance**](/specs) | Requirements from `.openspec/specs/` vs implementation |
        | [**Codebase Metrics**](/metrics) | Lines of code by module, growth over time |

        ---

        ## Quick Stats

        Use these dashboards to monitor:

        - **Backend parity** — Are both backends implementing the same stdlib functions?
        - **Phase progress** — Which phase are we in? How many issues remain?
        - **Epic status** — How close are the major epics to completion?
        - **Test health** — Are cross-backend tests passing?
        - **Spec coverage** — Which requirements have implementation links?

        ---

        *MVL Tracker v0.1.0*
        """
    )
    return


if __name__ == "__main__":
    app.run()
