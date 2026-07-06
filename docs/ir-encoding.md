# Strata/K IR JSON encoding convention (frozen at ir_version 0.1.0)

The Rust ADTs in `strata-ir` are the single source of truth; this document
records the **serde encoding convention** every type obeys so the public JSON
contract never gets retrofitted (IR-1, critic risk #1). LLM-writability is the
priority: the rules below are chosen so a model can emit valid High-IR from the
schema without ambiguity.

## Rules

1. **Field naming:** `snake_case` everywhere (`#[serde(rename_all = "snake_case")]`).
2. **Payload-bearing enums** (variants carry data): **adjacently tagged** —
   `#[serde(tag = "kind", content = "data", rename_all = "snake_case")]`.
   Serialized form: `{"kind": "rule", "data": { ... }}`. One rule for every such
   enum (Item, Term, Literal, Annotation, …) — chosen over internal tagging
   because it works uniformly for unit, newtype, and struct variants, so
   variant payloads can stay reusable structs.
3. **Pure-unit enums** (no variant carries data): bare `snake_case` **strings**
   via `#[serde(rename_all = "snake_case")]` (e.g. `AggOp` → `"min"`,
   `Termination` → `"total"`).
4. **Optional fields:** `#[serde(default, skip_serializing_if = "…is_empty/is_none")]`
   so absent = default and the JSON stays minimal.
5. **Trop weight** ([`trop::Weight`]) is the one hand-encoded type: a finite
   weight is a bare JSON integer (`5`), and +∞ is the JSON string `"inf"`.
   These never collide, so `(min,+)` comparison is bit-exact (D6).
6. **`ir_version`** is a required top-level string on every `Program` document.
7. **Compound terms** (`@terms`) follow rule 2 and nest recursively:
   `cons(X, nil)` is
   `{"kind": "compound", "data": {"functor": "cons", "args": [{"kind": "var",
   "data": {"name": "X"}}, {"kind": "const", "data": {"name": "nil"}}]}}`.

## Example (transitive closure, Bool)

```json
{
  "ir_version": "0.1.0",
  "items": [
    {"kind": "predicate", "data": {"name": "edge",
      "sig": {"args": ["node", "node"], "annotation": "bool", "effects": {}}}},
    {"kind": "predicate", "data": {"name": "path",
      "sig": {"args": ["node", "node"], "annotation": "bool", "effects": {}}}},
    {"kind": "rule", "data": {
      "head": {"pred": "path", "args": [{"kind": "var", "data": {"name": "X"}},
                                        {"kind": "var", "data": {"name": "Z"}}]},
      "body": [
        {"kind": "pos", "data": {"pred": "edge", "args": [{"kind": "var", "data": {"name": "X"}},
                                                          {"kind": "var", "data": {"name": "Y"}}]}},
        {"kind": "pos", "data": {"pred": "path", "args": [{"kind": "var", "data": {"name": "Y"}},
                                                          {"kind": "var", "data": {"name": "Z"}}]}}
      ]}}
  ]
}
```
