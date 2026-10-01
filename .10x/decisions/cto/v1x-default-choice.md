# CTO — v1.1 / v1.2 "the default choice" (2026-10-01)

**Problem.** v1 is complete and the launch material is live, but "a team could still say no"
for reasons we can enumerate: lifecycle edges (`WeakCtx`, silent off-runtime drops), an Android
dev loop that is not live, persisted state that does not survive schema changes, no devtools,
no story for teams on React Native, no structured local storage port, and gaps we have not
audited (boundary type coverage, platform parity).

**Decisions.**
1. Two discovery documents before new features: our own gap audit from the code, and a sourced
   catalogue of the alternatives' limitations. Features are scheduled from those tables, not from
   intuition; the same tables become the public "why Undra" post.
2. Runtime-model changes (`WeakCtx`, typed off-runtime writes, typed stream errors, persisted
   state migrations) each get an ADR before code (R11) and the strongest model implements and
   reviews them; everything else is cheap-model implementation behind strong reviews.
3. v1.2's reach bets (React Native runtime, devtools, `Db` port, WebSocket port, derived lists,
   Dart) are the founder's call; the plan is written so they slot in without reordering.
4. Distribution stays parked until after v2, as the founder asked.
