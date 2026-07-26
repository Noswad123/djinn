# Coven/Djinn interop fixtures

These fixtures exercise the first schema slice in
[`../../schemas/coven-djinn-event.schema.json`](../../schemas/coven-djinn-event.schema.json).

- `session-start-request.json`: Coven asks Djinn to start a background child
  session.
- `session-created-fact.json`: Djinn reports the accepted/created native session
  with runtime and presentation refs.
- `policy-grant-request.json`: Coven asks Djinn to apply a scoped session grant.
- `result-available-fact.json`: Djinn reports a concise result artifact without
  copying the full transcript into Coven.

These files are intended to become parser/contract test inputs once the bridge
types exist in code.
