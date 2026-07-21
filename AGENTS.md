# Repository Guidelines

## Project Structure & Module Organization

- `src/elamite/` contains the compiler frontend. `elx.py` is the command-line
  entry point; `transformer.py` builds AST nodes; `analyzer.py` performs semantic
  checks; and `elx_types.py` contains the core data types.
- `elamite.lark` is the Lark grammar loaded by `Elx`. Update it deliberately
  with the parser-facing code that depends on it.
- `examples/` holds Elamite source examples. `SPEC.md` is the language design;
  `ISSUES.md` records unresolved design work.
- `tests/` contains test scaffolding and `tests/test_proj/` holds a sample
  `.els` project. The current test modules are empty.

## Build, Test, and Development Commands

Use Python 3.13 and `uv`:

```sh
uv sync                                  # create/update the local environment
uv run python -m compileall src          # syntax-check Python sources
uv run python -m elamite.elx tests/test_proj  # exercise parsing and analysis
```

There is no configured automated test framework yet. When adding `pytest` as a
development dependency, run its suite with `uv run pytest`. The current sample
project intentionally reaches an analyzer error for an unresolved `&x` type, so
use its output as a diagnostic smoke check rather than a passing test.

## Coding Style & Naming Conventions

Follow standard Python style: four-space indentation, `snake_case` for modules,
functions, and variables, `PascalCase` for classes, and `UPPER_CASE` for module
constants. Preserve existing type annotations and `dataclass`/`Enum` patterns.
Keep parser, transformer, and analyzer responsibilities separate; do not embed
semantic checks in grammar actions. No formatter or linter is configured, so
match surrounding code and keep imports grouped: standard library, third-party,
then local package imports.

## Testing Guidelines

Add focused tests under `tests/test_<area>.py` and name cases
`test_<behavior>`. Cover both successful parsing/analysis and meaningful error
paths. Use `.els` fixtures under `tests/test_proj/` when a behavior is clearer
as source text. Include the exact command run in the pull request.

## Design, Commits, and Pull Requests

Treat `SPEC.md` and the authoritative `examples/spec_demo.elx` as design inputs.
Append new entries to `ISSUES.md`; use `I-X.Y` for a sub-issue related to
`I-X`. Do not change parser grammar while design review is active.

Recent commits use short, lowercase descriptive subjects, such as `repo
cleanup`. Keep commits focused and imperative. Pull requests should summarize
the behavior change, link relevant issues, identify grammar/spec impact, and
report validation performed. Include diagnostic output when changing errors.
