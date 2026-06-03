# snake_game

Terminal snake game — demonstrates **Req 7 sharp effect boundary** with pure game logic.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Effect separation | `game.mvl` = pure, `main.mvl` = `! Terminal + Clock` | Testable core |
| Terminal effect | `! Terminal` | Raw TUI control (distinct from Console) |
| Option handling | `Option[Direction]` | No key press on some ticks |
| Trust boundary | `extern "rust" { fn tui_init(), ... }` | TUI lifecycle in Rust |

---

## Module structure

| File | Effects | Purpose |
|------|---------|---------|
| `game.mvl` | None | Pure game state transitions |
| `render.mvl` | `! Terminal` | Drawing only |
| `input.mvl` | None | Key-to-direction mapping (pure) |
| `main.mvl` | `! Terminal + Clock` | Game loop, random food |

---

## Effect boundary check

```bash
grep '!' examples/snake_game/game.mvl
# (no output — pure file)

grep '!' examples/snake_game/main.mvl
# fn main() -> Unit ! Terminal + Clock
```

---

## Game loop

```
loop:
  1. tui_read_key_timeout(100ms) → Option[Direction]
  2. apply_update(game, direction) → Game (pure)
  3. render(game) ! Terminal
  4. check game_over → break or continue
```

---

## Running

```bash
make build
cd examples/snake_game
make run
```

Controls: Arrow keys to move, `q` to quit.

---

## Related

- Spec: `.openspec/specs/002-effect-system/spec.md`
- Trust boundary: `bridge.rs`
