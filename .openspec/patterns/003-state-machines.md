# Pattern 003: State Machines via Exhaustive Match

## Summary

Model state machines using ADTs and exhaustive match rather than a dedicated library.
MVL's type system (REQ1), exhaustive match (REQ3), ownership (REQ6), effect tracking
(REQ7), and refinement types (REQ10) together provide compile-time state machine
verification that most languages need a library for.

## When to use

- Workflow states (order processing, approval chains, maintenance lifecycle)
- Protocol states (connection handshake, authentication flow)
- Entity lifecycle (created → active → archived)
- Any domain where "forgot to handle this combination" is a real risk

## When NOT to use

- Hierarchical state machines (nested substates, history, parallel regions) — these
  need richer structure than a flat enum
- Runtime-configurable transitions (rules loaded from config/database)

## Approach 1: Transition function (simple)

The match expression *is* the transition table. Suitable when invalid transitions
are expected and handled as `None`.

```mvl
enum State { Idle, Running, Paused, Done }
enum Event { Start, Pause, Resume, Finish }

total fn transition(s: State, e: Event) -> Option[State] {
    match (s, e) {
        (Idle,    Start)  => Some(Running),
        (Running, Pause)  => Some(Paused),
        (Running, Finish) => Some(Done),
        (Paused,  Resume) => Some(Running),
        _                 => None
    }
}
```

Trade-off: wildcard `_` swallows invalid transitions. Adding a new variant to
`State` or `Event` does NOT trigger a compile error — the wildcard absorbs it.

## Approach 2: Explicit rejection (recommended)

Every state-event pair has an explicit decision. Adding a new variant forces the
compiler (REQ3) to demand handling for every new combination.

```mvl
enum State { Idle, Running, Paused, Done }
enum Event { Start, Pause, Resume, Finish }

enum TransitionError {
    InvalidTransition { from: State, event: Event }
}

total fn transition(s: State, e: Event) -> Result[State, TransitionError] {
    match (s, e) {
        // valid transitions
        (Idle,    Start)  => Ok(Running),
        (Running, Pause)  => Ok(Paused),
        (Running, Finish) => Ok(Done),
        (Paused,  Resume) => Ok(Running),

        // explicit rejections
        (Idle,    Pause)  => Err(InvalidTransition(s, e)),
        (Idle,    Resume) => Err(InvalidTransition(s, e)),
        (Idle,    Finish) => Err(InvalidTransition(s, e)),
        (Done,    _)      => Err(InvalidTransition(s, e)),
        (Paused,  Start)  => Err(InvalidTransition(s, e)),
        (Paused,  Pause)  => Err(InvalidTransition(s, e)),
        (Paused,  Finish) => Err(InvalidTransition(s, e)),
        (Running, Start)  => Err(InvalidTransition(s, e)),
        (Running, Resume) => Err(InvalidTransition(s, e)),
    }
}
```

The verbosity is the value: every combination is a conscious decision. Add a sixth
event and the compiler emits errors until all new pairs are handled.

## Approach 3: Data table (for visualization/serialization)

Transitions as data — readable, diffable, serializable for diagram generation.
Loses compile-time exhaustiveness; a missing rule is a runtime `None`.

```mvl
struct Rule { from: State, event: Event, to: State }

let rules: List[Rule] = [
    Rule { from: Idle,    event: Start,  to: Running },
    Rule { from: Running, event: Pause,  to: Paused  },
    Rule { from: Running, event: Finish, to: Done    },
    Rule { from: Paused,  event: Resume, to: Running },
]

fn transition(s: State, e: Event) -> Option[State] {
    rules.find(|r| r.from == s && r.event == e).map(|r| r.to)
}
```

Use when: transitions are loaded from configuration, need to be serialized for
external tools, or you want to generate diagrams from the table.

## Combining with other MVL features

### Effects per state

Different states can permit different side effects:

```mvl
fn handle(s: State, e: Event) -> Result[State, TransitionError] with IO {
    let next = transition(s, e)?
    match next {
        Running => log("started"),   // IO effect
        Done    => log("finished"),  // IO effect
        _       => ()                // no effect
    }
    Ok(next)
}
```

### Ownership for linear state consumption

Ownership (REQ6) ensures the old state is consumed — you cannot accidentally
use a stale state after a transition:

```mvl
fn step(s: State, e: Event) -> Result[State, TransitionError] {
    // s is moved into transition — cannot be used after this
    transition(s, e)
}
```

### Refinement types for state-dependent constraints

```mvl
total fn run_task(s: State where s == Running, task: Task) -> State {
    // only callable when in Running state — compile-time enforced
    if task.is_complete() { Done } else { Running }
}
```

## Recommendation

Use **Approach 2** (explicit rejection) as the default. The compile-time
exhaustiveness guarantee is MVL's differentiator — a state machine library
would trade this guarantee for syntactic convenience, which is the wrong
trade-off for safety-critical domains.

Use Approach 3 only when transitions must be runtime-configurable or when
external tooling (diagram generators, audit reports) needs the table as data.

## Related

- REQ1 (Type Safety) — ADTs model states
- REQ3 (Exhaustive Match) — compiler forces all pairs handled
- REQ5 (Error Visibility) — `Result` for transition errors
- REQ6 (Ownership) — consumed state prevents stale references
- REQ7 (Effect Tracking) — per-state effect discipline
- REQ10 (Refinement Types) — state-dependent function preconditions
