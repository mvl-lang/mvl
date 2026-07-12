# MVL Pong — Requirements

Formal spec for `examples/pong` — the classic paddle-and-ball game, built to
demonstrate all 11 MVL requirements and give the refinement/contract prover
significant load.

Version: 0.1.0 (draft, pre-implementation)
Last updated: 2026-07-12

---

## 1. Intent

Build a terminal Pong that:

1. Uses `pkg.tui` for raw-mode I/O (mirrors `examples/snake_game`).
2. Follows the `crud_api` file layout convention (types / logic / I/O split
   into separate files, each with a paired `_test.mvl`).
3. Exercises **all 11 MVL requirements** — not just Req 7 (effects) like
   snake_game, but also Req 10 (refinements + contracts) heavily so the
   prover does real work.
4. Ships two play modes and two visual palettes.
5. Ships three AI difficulty levels that drive ball speed policy.
6. Opens with an interactive menu, or accepts CLI args to skip the menu.

## 2. Non-goals

- No sound, no smooth animation, no networking.
- No config file (`config.toml`), no persistence, no save/load.
- No multi-ball, no power-ups, no obstacles.
- No spin physics — bounces are angle-of-incidence = angle-of-reflection
  with a small `vy` perturbation on paddle-edge hits.
- No IFC labels (`Secret[T]`, `Tainted[T]`) — a game has no secrets to protect.
  Req 11 is exercised trivially (no violations to catch).

## 3. Play modes and options

### 3.1 Mode
```mvl
pub type Mode = enum {
    SinglePlayer,   // human left, AI right
    TwoPlayer,      // human left (W/S), human right (↑/↓)
}
```

### 3.2 Palette
```mvl
pub type Palette = enum {
    BlackWhite,     // monochrome — everything Style::White
    Color,          // left = Cyan, right = Magenta, ball = Yellow, borders = White
}
```

### 3.3 Difficulty
```mvl
pub type Difficulty = enum {
    Simple,   // |vx| = 1 forever
    Medium,   // |vx| += 1 every 3 paddle bounces, capped at 3
    Hard,     // |vx| += 1 every point scored, capped at 3
}
```

### 3.4 Side (used in scoring and win detection)
```mvl
pub type Side = enum { Left, Right }
```

### 3.5 Game status
```mvl
pub type GameStatus = enum {
    Playing,
    Won(Side),
}
```

## 4. File layout

```
examples/pong/
├── mvl.toml              — package manifest + pkg-tui dep
├── mvl.lock
├── Makefile              — check / run / test-mvl / mcdc / prove
├── README.md             — quickstart + original spec instruction verbatim
├── requirements.md       — this file
├── LICENSE               — Apache-2.0
├── models.mvl            — types, refinements, invariants
├── game.mvl              — pure game logic (all `total fn`)
├── input.mvl             — Key → PaddleInput (pure)
├── main.mvl              — menu + CLI parse + game loop + rendering (effects)
├── models_test.mvl       — constructor + invariant tests (~6)
├── game_test.mvl         — physics, AI, scoring, win tests (~25)
└── input_test.mvl        — key mapping tests (~8)
```

## 5. MVL Requirement mapping (all 11 covered)

| # | MVL Requirement | How Pong exercises it |
|---|---|---|
| 1 | **Type Safety** | ADTs for every domain concept — `Ball`, `Paddle`, `Field`, `Game`, `Mode`, `Palette`, `Side`, `Difficulty`, `PaddleInput`, `GameStatus`. No primitive obsession. |
| 2 | **Memory Safety** | All value types; no `ref` cycles. `Terminal` is the only owned resource, dropped in `main` via RAII. |
| 3 | **Exhaustiveness** | Every `match` on `Mode`, `Palette`, `Difficulty`, `PaddleInput`, `Side`, `GameStatus` covers every arm — no `_` wildcards in `game.mvl`. |
| 4 | **Null Elimination** | `Option[PaddleInput]` for "no key this tick". Zero bare `unwrap()`. Every `Option`/`Result` handled with `match` or `?`. |
| 5 | **Error Visibility** | `new_terminal()` returns `Result`; propagated via `?` in `main`. CLI parse returns `Result[Config, ParseError]`. |
| 6 | **Ownership** | `Terminal` is `iso` (from pkg-tui). Game state passed by value; `val` borrows used for read-only paddle/field access. |
| 7 | **Effect Tracking** | Sharp boundary — `models.mvl`, `game.mvl`, `input.mvl` = zero effects. `main.mvl` = `! Terminal + Random + Console + Args`. |
| 8 | **Termination** | Every function in `game.mvl` / `input.mvl` / `models.mvl` marked `total fn`. Game loop marked `partial fn` (user-driven). |
| 9 | **Data Race Freedom** | Single-threaded. No actors, no shared state. Trivially satisfied. |
| 10 | **Refinement & Contracts** | Heavy — see §6 and §7 for the full list of proof obligations. |
| 11 | **Information Flow** | Trivial — no `Secret`/`Tainted` labels. Documented as satisfied by absence of violations. |

## 6. Refinement types (Req 10, part 1)

| Type | Refinement | Rationale |
|---|---|---|
| `Field.width` | `Int where self >= 20 && self <= 120` | Renderable range; prover discharges bounds at construction |
| `Field.height` | `Int where self >= 10 && self <= 40` | Same, vertical |
| `Ball.x` | `Int where self >= 0 && self <= 120` | In-field horizontal |
| `Ball.y` | `Int where self >= 0 && self <= 40` | In-field vertical |
| `Ball.vx` | `Int where self >= -3 && self <= 3 && self != 0` | Non-zero, bounded magnitude |
| `Ball.vy` | `Int where self >= -1 && self <= 1` | Angle only |
| `Paddle.y` | `Int where self >= 0 && self <= 40` | Top of paddle within field |
| `Paddle.height` | `Int where self >= 2 && self <= 8` | Sensible paddle size |
| `Game.left_score` | `Int where self >= 0 && self <= 11` | Non-negative, capped at winning score |
| `Game.right_score` | `Int where self >= 0 && self <= 11` | Same |
| `Game.rally_bounces` | `Int where self >= 0` | Non-negative rally counter |
| `Config.winning_score` | `Int where self >= 1 && self <= 21` | Configurable win threshold, default 11 |

## 7. Contracts (Req 10, part 2)

Contracts on the pure functions in `game.mvl`. Every one becomes a proof
obligation for the Z3 solver.

### 7.1 `new_game(field, mode, difficulty) -> Game`
```mvl
ensures result.status == GameStatus::Playing
ensures result.left_score == 0
ensures result.right_score == 0
ensures result.rally_bounces == 0
ensures result.ball.x == field.width / 2
ensures result.ball.y == field.height / 2
ensures result.left_paddle.y  == (field.height - result.left_paddle.height) / 2
ensures result.right_paddle.y == (field.height - result.right_paddle.height) / 2
ensures result.mode == mode
ensures result.difficulty == difficulty
```

### 7.2 `step_ball(ball, field) -> Ball`
```mvl
requires ball.x >= 0 && ball.x < field.width
requires ball.y >= 0 && ball.y < field.height
ensures  result.x >= 0 && result.x < field.width
ensures  result.y >= 0 && result.y < field.height
ensures  abs(result.vx) == abs(ball.vx)     // magnitude preserved unless bounced (see 7.3)
```

### 7.3 `bounce_wall(ball, field) -> Ball`
```mvl
ensures result.vx == ball.vx                 // horizontal unchanged
ensures abs(result.vy) == abs(ball.vy)       // magnitude preserved
ensures (ball.y == 0 || ball.y == field.height - 1) implies result.vy == -ball.vy
ensures (ball.y != 0 && ball.y != field.height - 1) implies result.vy == ball.vy
```

### 7.4 `bounce_paddle(ball, paddle, side) -> Ball`
```mvl
ensures abs(result.vx) >= abs(ball.vx)       // may speed up (medium/hard) but never slow
ensures abs(result.vx) <= 3                  // cap
ensures result.vx * ball.vx <= 0             // direction flipped or was already opposite
ensures result.y == ball.y                   // y position unchanged by paddle bounce
```

### 7.5 `move_paddle_by_input(paddle, input, field) -> Paddle`
```mvl
ensures abs(result.y - paddle.y) <= 1        // at most one row per tick
ensures result.y >= 0
ensures result.y + paddle.height <= field.height
ensures result.height == paddle.height       // unchanged
```

### 7.6 `ai_move(paddle, ball, field) -> Paddle`
```mvl
ensures abs(result.y - paddle.y) <= 1        // AI also bounded per tick
ensures result.y >= 0
ensures result.y + paddle.height <= field.height
```

### 7.7 `resolve_scoring(game, field) -> Game`
```mvl
ensures result.left_score >= game.left_score
ensures result.right_score >= game.right_score
ensures (result.left_score + result.right_score) - (game.left_score + game.right_score) <= 1
ensures result.ball.x == field.width / 2 || result.status != GameStatus::Playing
```

### 7.8 `check_win(game, winning_score) -> GameStatus`
```mvl
requires winning_score >= 1 && winning_score <= 21
ensures result == GameStatus::Won(Side::Left)  || game.left_score  >= winning_score
     || result == GameStatus::Won(Side::Right) || game.right_score >= winning_score
     || result == GameStatus::Playing
```

### 7.9 `speed_step_up(ball, difficulty, event) -> Ball`
```mvl
ensures abs(result.vx) >= abs(ball.vx)       // monotone up
ensures abs(result.vx) <= 3                  // cap
ensures result.y == ball.y && result.vy == ball.vy   // only vx changes
```

### 7.10 `field_from_terminal(size) -> Field`
```mvl
ensures result.width  >= 20 && result.width  <= 120
ensures result.height >= 10 && result.height <= 40
```

**Total explicit `requires`/`ensures` contracts: ~30**, plus refinement
discharges at every literal / construction site.

## 8. Struct invariants (Req 10, part 3)

Some invariants are best expressed at the type level:

```mvl
pub type Field = struct {
    width:  Int where self >= 20 && self <= 120,
    height: Int where self >= 10 && self <= 40,
}

pub type Paddle = struct {
    y: Int where self >= 0 && self <= 40,
    height: Int where self >= 2 && self <= 8,
} with invariant self.y + self.height <= 40  // always fits somewhere in max field
```

The `with invariant` on `Paddle` is proved at every construction site.

## 9. Test matrix

### `models_test.mvl` — ~6 tests
- Constructing `Ball` / `Paddle` / `Field` at each boundary succeeds.
- Constructing a `Field` outside `[20..120] × [10..40]` fails.
- Paddle `y + height` invariant fires when violated.

### `game_test.mvl` — ~25 tests
- `new_game` returns centered ball, zero scores, Playing status.
- `step_ball` moves by `(vx, vy)` when no collision.
- `bounce_wall` flips `vy` at top and bottom only.
- `bounce_paddle` flips `vx` on contact.
- `bounce_paddle` speed-ups: Simple never speeds up, Medium after 3 bounces, Hard after a point.
- `resolve_scoring` awards point when ball crosses left/right edge.
- `resolve_scoring` resets ball to center after scoring.
- `resolve_scoring` awards exactly one point per crossing.
- `check_win` returns `Won(Left)` when left reaches 11.
- `check_win` returns `Won(Right)` when right reaches 11.
- `ai_move` tracks ball vertically (Δy ≤ 1).
- `move_paddle_by_input` respects field bounds at top and bottom.
- Mode-specific: SinglePlayer uses `ai_move`; TwoPlayer uses input for both paddles.

### `input_test.mvl` — ~8 tests
- `↑` → `Some(PaddleInput::Up)`
- `↓` → `Some(PaddleInput::Down)`
- `W` → `Some(PaddleInput::Up)` (left player key)
- `S` → `Some(PaddleInput::Down)`
- Unknown key → `None`
- `Esc` → distinct `MetaInput::Quit` (via a different function)

### MC/DC targets
- 100% on `step_ball`, `bounce_wall`, `bounce_paddle`, `resolve_scoring`,
  `check_win`, `ai_move`, `speed_step_up`.

## 10. CLI reference

```
Usage: pong [OPTIONS]

Options:
  --mode {single,two}                Play mode (default: prompt via menu)
  --palette {bw,color}               Visual palette (default: prompt via menu)
  --difficulty {simple,medium,hard}  AI difficulty / speed policy (default: prompt via menu)
  -h, --help                         Show this help
```

Behavior:
- All three of `--mode` / `--palette` / `--difficulty` provided → skip menu.
- Any missing → menu opens pre-filled with defaults (`Single`, `Color`, `Medium`).
- Invalid value → error message + non-zero exit.

## 11. Menu wireframe

```
╔══════════════════════════════════════════════╗
║               M V L   P O N G                ║
╠══════════════════════════════════════════════╣
║                                              ║
║   Mode:       [ Single ]   Two               ║
║   Palette:      B/W      [ Color ]           ║
║   Difficulty:  Simple  [ Medium ]  Hard      ║
║                                              ║
║   ↑↓ change · ←→ switch · ⏎ start · Esc quit ║
╚══════════════════════════════════════════════╝
```

- `↑`/`↓` — move between rows (Mode, Palette, Difficulty).
- `←`/`→` — cycle the value in the current row.
- `⏎` — commit and start the game.
- `Esc` — abort and exit cleanly.

## 12. Effect boundary

| File | Effects | Rationale |
|---|---|---|
| `models.mvl` | *(none)* | Pure types |
| `game.mvl` | *(none)* | Pure logic; all `total fn` |
| `input.mvl` | *(none)* | Pure `Key → PaddleInput` mapping |
| `main.mvl` | `! Terminal + Random + Console + Args` | Menu, loop, rendering, CLI parse |

This split is the single most important design decision — it keeps ~90% of
the code fully testable without a TTY, and the prover proves it stays that
way because effect annotations propagate.

## 13. Ball direction on new game / after score

- `Ball.vy` starts at `0` (pure horizontal).
- `Ball.vx` starts at `+1` on new game, `-1` on left-scores (serves toward
  the loser), `+1` on right-scores.
- On paddle hit at the paddle's ends, `vy` may become `±1`; otherwise stays
  `0` — this simple angle model keeps physics proofs tractable.

## 14. Rendering

- Border: `╔═╗╚═╝║` box glyphs (mono in B/W, White in Color).
- Ball: `●` (color: Yellow in Color mode, White in B/W).
- Paddles: `█` stacked `Paddle.height` rows (Cyan / Magenta in Color, White in B/W).
- Score bar: `Left: N     Right: M` centered above the field, `⏎ to quit` below.
- On win: overlay `PLAYER 1 WINS` / `PLAYER 2 WINS` / `YOU WIN` / `YOU LOSE`
  centered, wait for keypress, exit.

## 15. Makefile targets

The `Makefile` exposes the full quality gate as one-word targets. All targets
run against the local `examples/pong/` sources — no CI-only paths.

| Target | Command | Purpose |
|---|---|---|
| `make check` | `mvl check .` | Type-check + refinement bounds (no proofs) |
| `make prove` | `mvl prove . --verbose` | Discharge every `requires`/`ensures` via Z3 |
| `make test-mvl` | `mvl test .` | Run all `*_test.mvl` unit tests |
| `make coverage` | `mvl test . --coverage` | Branch coverage report |
| `make mcdc` | `mvl mcdc . --verbose` | MC/DC condition coverage report |
| `make assurance` | `mvl assurance . --json` | Full ISPE-style assurance report (11-req roll-up) |
| `make run` | `mvl run main.mvl` | Launch the game (menu, then play) |
| `make all` | `check test-mvl coverage mcdc prove assurance` | Full quality gate — CI-equivalent |

`make all` is the default `.PHONY: all` target. Every commit that touches
this example is expected to pass it locally.

## 16. Definition of done

- `make check` — passes with zero errors.
- `make prove` — every `requires`/`ensures` and every refinement discharge
  succeeds (≥30 proof obligations, all Z3-verified).
- `make test-mvl` — all tests pass.
- `make coverage` — ≥90% branch coverage on `game.mvl`, `models.mvl`, `input.mvl`.
- `make mcdc` — 100% coverage on the seven MC/DC targets listed in §9.
- `make assurance` — all 11 MVL requirements reported as satisfied.
- Manual smoke test: `make run` — menu appears, game plays, both palettes
  render correctly, all three difficulties behave differently, both
  single/two-player work.
