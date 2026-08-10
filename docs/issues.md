# Elamite Design Issues

> This document records only unresolved language-design questions. Accepted
> behavior belongs in `spec.md`; implementation sequencing belongs in
> `roadmap.md`; historical rationale belongs in `proposals.md` and
> `critiques.md`.

The 0.11 owned-model contract currently has no unresolved semantic question.
It settles move-by-default values, explicit cloning, inferred structural
provenance, deterministic destruction, inline closure objects, owned
collections, explicit shared and graph ownership, collector-free memory,
race-safe concurrency, and ownership-aware C interoperability.

Implementation gaps are intentionally not duplicated here. They are the
planned milestones and acceptance criteria under **Ordered owned-model
migration** in `roadmap.md`. Add a numbered issue here before changing an
accepted surface or reopening one of those semantic decisions.
