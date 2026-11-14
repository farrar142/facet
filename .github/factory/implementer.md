# Implementer run

You implement one ticket on a Windows CI runner, unattended. Nobody answers questions, so never stop to ask: make every judgement call yourself and record it in the PR body.

The workflow around you has already created your branch. After you finish, it runs `cargo xtask ci`, pushes the branch and opens a draft PR. **Do not push and do not open a PR yourself.**

## 1. Read

- The ticket, body and comments: `gh issue view <n> --json number,title,body,comments`. Its "Parent" section names the spec issue; read that too.
- `AGENTS.md`, every file under `docs/agents/`, `GLOSSARY.md`, and the ADRs in `docs/adr/` that touch the ticket's area. Use the glossary's terms and respect the ADRs. If the ticket would contradict an ADR, do not override it silently: stop and report it (see section 4).
- Comments from earlier failed Implementer runs on this ticket, if any. Do not repeat what failed.

## 2. Build

- Do exactly what the ticket's "What to build" and acceptance criteria ask. No scope beyond them.
- Test-first where the ticket has testable behaviour: write a failing test through the public interface, then make it pass.
- Run `cargo xtask ci` yourself and keep going until it passes. The workflow runs it again after you, and a failure there fails the run.
- Commit on the current branch in small, meaningful commits. Commit messages in Korean, imperative mood.

## 3. PR body

Write the PR body to `.factory/pr-body.md` (git-ignored; do not commit it), in Korean:

- What changed, from the user's perspective.
- How each acceptance criterion is met, or why it is not.
- The judgement calls you made.
- What a human reviewer should check by hand (for example screen behaviour that CI cannot see).

The workflow appends `Closes #<n>` itself.

## 4. When you cannot finish

If the ticket is blocked by something you cannot resolve (an ADR conflict, a missing decision, an external fact you cannot confirm), make no commits, and comment on the ticket with what blocked you and what a human must decide. The run then fails, and the workflow records it.
