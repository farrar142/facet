# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Edit the "Label in our tracker" column to match whatever vocabulary you actually use.

## Factory labels (facet only)

These are not triage roles. The software factory uses them (see `docs/adr/0001-software-factory.md`).

| Label         | Meaning                                                                                     |
| ------------- | ------------------------------------------------------------------------------------------- |
| `spec`        | A spec issue. Triggers the Decompose task. A spec issue never gets `ready-for-agent`, even when `/to-spec` says to apply it. |
| `in-progress` | An Implementer run has claimed this ticket. The run removes `ready-for-agent` and adds this. |
