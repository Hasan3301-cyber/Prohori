# First-aid protocol schema

One JSON file per protocol. The filename stem **is** the `id` — `cpr.adult.json` holds
`"id": "cpr.adult"` — and `core/tests/corpus_integrity.rs` fails the build if they
disagree.

This corpus is global. It does not live in a city pack: a new country costs a pack, not
a corpus (`PLAN.md` §6).

## Why this file is strict

The model is not allowed to author medical content (`PLAN.md` §1). That is only
meaningful if there is a source of truth for it to be checked against, so every number,
every instruction, and every warning a user ever sees has to exist here first.
`core/src/verifier.rs` compares rendered output against `Protocol::source_text` and
rejects anything that appeared out of nowhere.

## Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `id` | string | yes | Matches the filename stem. Stable forever; referenced by `redflag::RULES`. |
| `version` | string | yes | Bump on any content change, including wording. |
| `title` | string | yes | Shown as the card heading. Plain words. |
| `applies_to` | string | yes | Who this is for, in one sentence. The reader checks themselves against it. |
| `also_called` | array of strings | no | Lay search words. Indexed, never shown. See below. |
| `reading_grade` | integer | yes | Flesch–Kincaid grade of the step text. `PLAN.md` §8 gates this at ≤ 6. |
| `reviewed_by` | string or null | yes | Name and credential of the clinician who signed off. `null` until one has. |
| `reviewed_at` | string or null | yes | ISO 8601 date. `null` with `reviewed_by`. |
| `citations` | array | yes, ≥ 1 | Where the content comes from. |
| `steps` | array | yes, ≥ 1 | Ordered. See below. |
| `do_not` | array of strings | no | Things that make it worse. Rendered as a separate block, never mixed into steps. |
| `escalate_if` | array of strings | no | Conditions that change the answer mid-protocol. |

### `citations[]`

| Field | Type | Required |
|---|---|---|
| `source` | string | yes |
| `section` | string | no |
| `url` | string | no |

### `steps[]`

| Field | Type | Required | Notes |
|---|---|---|---|
| `n` | integer | yes | 1-based, contiguous, ascending. Gaps fail validation. |
| `kind` | `assessment` \| `action` \| `escalation` | yes | See below. |
| `text` | string | yes | One instruction. Non-empty. |

## `kind` is load-bearing, not documentation

`assessment` means *look, listen, ask* — nothing has been done to the patient yet.
`action` means *do this to them*. `escalation` means *get more help than you are*.

Two invariants depend on the distinction:

1. **A protocol may not open with an `action`.** Step 1 is always an `assessment` or an
   `escalation`. A card that opens by telling a frightened person to press on a chest,
   before they have checked whether they are looking at the situation the card is for,
   is a card that can hurt someone who arrived at it by mistake.

2. Specifically, `cpr.adult` step 1 is an `assessment`. The red-flag layer deliberately
   overtriages "I can't breathe" onto the CPR card (`core/src/redflag.rs`, "Known,
   deliberate overtriage"), and accepting that overtriage is only defensible because the
   first thing the card does is tell the reader to check for a response. A talking
   patient fails that check in two seconds. Both invariants are asserted in
   `core/tests/corpus_integrity.rs`.

## `also_called` is the search field, and it is not content

`core/src/retrieval.rs` ranks these files against whatever the user typed. The card that
covers cardiac arrest is titled "Not breathing — push on the chest", and nobody types
that. They type *heart attack*, *collapsed*, *no pulse*, *he is not waking up*. Those
phrasings have to live somewhere, and the two other candidates are both worse:

- In `core/src/normalize.rs`, as spelling folds. Forbidden by `docs/CONVENTIONS.md` §6 —
  `heart attack → cpr` is folding one word onto a different word, and it would fire on
  "she had a heart attack last year" with nobody able to see why from the rule table.
- In `core/src/redflag.rs`, as triggers. That layer is for the phrases that must bypass
  everything, and padding it with lower-confidence vocabulary makes the safety-critical
  list harder to review.

So they live here, next to the content they point at, where an author adding a card adds
its search words in the same commit.

Two rules follow from the field being invisible:

1. **It is never rendered.** Not in `renderable_text`, not in `render_verbatim`, not on
   screen. A rendering may not draw a word from it.
2. **It may not contain prose.** `Protocol::validate` refuses an entry over six words or
   one carrying `.`, `!`, `?`, or `;`. Text nobody displays is text nobody reviews and
   nobody grades, so an instruction smuggled in here would escape every check in this
   repository. Keeping the field to search phrases makes that structurally impossible
   rather than merely discouraged.

## Provenance

`reviewed_by: null` is the honest state for every protocol in this repository right now.
Content is transcribed from the cited international guidelines and has **not** been
signed off by a named clinician. Per `docs/CONVENTIONS.md` §9 that renders in the UI as
exactly that sentence — it does not render as nothing, and it is not quietly filled in
with "WHO" to make a screenshot look better.
