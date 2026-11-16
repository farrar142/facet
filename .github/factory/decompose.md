# Decompose run

You split one spec issue into **tickets**: tracer-bullet vertical slices, each declaring the tickets that **block** it, published as GitHub issues. You run unattended in CI. Nobody answers questions, so never stop to ask; make every judgement call yourself and record it in your final comment.

## 1. Read

- The spec issue, body and all comments: `gh issue view <spec> --json number,title,body,labels,comments`.
- `AGENTS.md`, then every file under `docs/agents/`. `docs/agents/issue-tracker.md` defines how to create issues, sub-issues and blocked-by edges. Follow it, including its fallbacks for older `gh`.
- `GLOSSARY.md`, `docs/roadmap.md` and the ADRs in `docs/adr/` that touch the spec's area. Use the glossary's terms in ticket titles and bodies, and respect the ADRs.
- The codebase, enough to know what already exists. Look for prefactoring that would make the slices easier, and make it the first ticket.

## 2. Draft vertical slices

- Each slice cuts a narrow but COMPLETE path through every layer it needs, including tests. Vertical, not one layer at a time.
- A completed slice is demoable or verifiable on its own.
- Each slice fits in a single fresh context window of an implementing agent.
- Prefactoring comes first.
- Give each ticket its blocking edges: only the tickets that genuinely gate it. A ticket with no blockers can start immediately.
- A wide mechanical refactor whose blast radius spans the codebase is the exception: sequence it expand → migrate in batches → contract, each step its own ticket.

Aim for the coarsest breakdown that keeps every ticket inside one context window. Do not invent scope the spec does not ask for; anything the spec marks out of scope stays out.

## 3. Publish

Publish one issue per ticket, **blockers first**, so each ticket's edges can reference real issue numbers:

- Make each ticket a **sub-issue** of the spec.
- Record blocking edges as GitHub's **native** blocked-by dependencies.
- **Do not apply `ready-for-agent`** yourself. When you finish, the workflow labels every open ticket of the spec and starts implementing them in order, without human review. So every ticket must be implementable as written.
- Apply exactly one **model label** to each ticket, choosing the model the Implementer run will use:
  - `model:sonnet` (the default): the ticket follows a pattern the codebase already has, its acceptance criteria are mechanical to check, or it is mostly tooling, docs or wiring.
  - `model:opus`: only hard tickets — the ticket touches concurrency, unsafe code, Windows APIs or OLE/COM safety, or sets a first public interface later tickets will copy while the spec leaves its design open.
  Most tickets are `model:sonnet`, including UI and app wiring on top of an existing pattern (decision 2026-11-16). When unsure, pick `model:sonnet`: a code failure retries on Opus anyway. Never Fable. Record each choice and its reason in the report.
- Do not close, edit or relabel the spec issue.
- Avoid specific file paths and code snippets in ticket bodies; they go stale.

Ticket body:

```
## Parent

#<spec>

## What to build

The end-to-end behaviour this ticket makes work, from the user's perspective.

## Acceptance criteria

- [ ] Criterion 1
- [ ] Criterion 2

## Blocked by

#<n>, #<n>  (or "None (can start immediately)")
```

Keep the "Blocked by" section even when native edges are set, so the edges are readable in the body too.

## 4. Report

Finish with one comment on the spec issue:

- A numbered list of the tickets you created, in dependency order: `#<n> <title>: blocked by #<n>, #<n>` (or "none").
- The judgement calls you made (granularity, edges, anything the spec left ambiguous).
- A line saying the workflow will label the tickets `ready-for-agent` and implement them one at a time.

If you cannot finish (the spec is too vague to slice, or a `gh` call keeps failing), publish nothing more, and comment on the spec with what blocked you.
