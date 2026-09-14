# Multi-instance tracker core

Status: ready for ticketing

This feature defines the initial `yaait` architecture, multi-instance tracker
model, JSON-first CLI contract, setup and secret-isolation flow, provider-neutral
usage reports, and the first GitHub Copilot provider.

- [Specification](spec.md)
- No source issues were folded into this feature.
- Non-blocking future questions are recorded in the specification.
- 2026-09-14: review decisions incorporated. V1 uses weak setup crash
  consistency, one application writer lock, compact command response schemas,
  warning-and-skip registry discovery, and numeric metrics only.
