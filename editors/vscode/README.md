# Elamite for VS Code

Syntax highlighting for `.elx` files. This extension is declarative only: a
TextMate grammar and a language configuration, with no compiled code and no
dependencies.

## Installing locally

Symlink this directory into your VS Code extensions folder, then run
`Developer: Reload Window` from the command palette. The folder name must
include a version suffix.

The correct folder depends on how VS Code reaches these files:

| Setup | Extensions folder |
| --- | --- |
| VS Code running natively | `~/.vscode/extensions` |
| Remote-WSL, Dev Containers, or SSH | `~/.vscode-server/extensions` |

```sh
# Remote-WSL, from the repository root:
ln -sfn "$PWD/editors/vscode" ~/.vscode-server/extensions/elamite-0.1.0
```

A symlink means edits to the grammar take effect on the next window reload, so
there is nothing to reinstall while iterating. To uninstall, delete the link.

The extension declares `extensionKind: ["ui", "workspace"]`, so it works
whichever side it is installed on.

To iterate with a debugger instead, open `editors/vscode/` in VS Code and press
F5 for an Extension Development Host. Either way, run
`Developer: Inspect Editor Tokens and Scopes` from the command palette to see
which rule produced a given token.

To produce a `.vsix`, run `npx @vscode/vsce package` from this directory.

## What it covers

- `//` line comments and `///` documentation comments
- Every keyword in `keyword` (`src/lexer.rs`)
- Primitive types from `SPEC.md` 4.1, plus the resolver's builtin types,
  derivable traits, and `print`/`println`
- Integer, float, hexadecimal, octal, and binary literals with `_` separators
  and every suffix in `parse_numeric_suffix`
- Strings, character literals, and escapes including `\u{...}`
- Formatted strings, with interpolation highlighted as real expressions,
  nested braces balanced, and `{{`/`}}` treated as literal braces
- `@importc`/`@exportc` attributes and `@vec`/`@map`/`@set` macros
- Declaration names for `fn`, `struct`, `enum`, `trait`, `type`, and `mod`
- Operators and punctuation

`language-configuration.json` also drives indentation: a line ending in `:`
increases indent, `else` decreases it, and Enter continues a `///` comment.

## Keyword coloring

Every keyword family sits under the `keyword.control` scope prefix, so
declarations and modifiers take the same color as `return` and `for` rather
than the separate `storage.type` color most grammars give them:

| Keywords | Scope |
| --- | --- |
| `if` `else` `while` `for` `in` `match` `return` `break` `continue` `defer` `pass` | `keyword.control.elx` |
| `fn` `struct` `enum` `trait` `impl` `type` `mod` `use` | `keyword.control.declaration.elx` |
| `pub` `unsafe` `let` `var` | `keyword.control.modifier.elx` |

Themes select scopes by dot-separated prefix, so all three resolve to whatever
a theme assigns `keyword.control` while staying individually overridable:

```jsonc
"editor.tokenColorCustomizations": {
  "textMateRules": [
    {
      "scope": "keyword.control.declaration.elx",
      "settings": { "foreground": "#569CD6" }
    }
  ]
}
```

Three reserved words deliberately sit outside this scheme: `as` is
`keyword.operator.word.elx`, `true`/`false`/`null` are `constant.language.elx`,
and `self`/`Self`/`super`/`root` are `variable.language.elx`.

## Known limits

These are inherent to grammar-based highlighting, which pattern-matches text
and has no knowledge of the program:

- Type versus value is a capitalization heuristic. `Point` and enum variants
  such as `Some` both scope as types; a lowercase name followed by `(` scopes
  as a call. Correct classification needs name resolution.
- `&` and `*` scope as the bitwise-and and multiplication operators even in
  reference and pointer types, because only a parser can tell those apart.
- `for` in `impl Trait for Type` scopes as the control keyword it shares a
  spelling with.
- Unknown `@name(...)` attributes are scoped `invalid.illegal`, matching the
  resolver rejecting them. `tests/editor_grammar.rs` keeps that list honest.

Diagnostics, hover, and go-to-definition are out of scope here; they would
require a language server built over the compiler.

## Keeping it in sync

The grammar necessarily duplicates lists that `src/` already owns.
`tests/editor_grammar.rs` compares them on every `cargo test` run and fails if
a keyword, numeric suffix, builtin, attribute, or macro is added to the
compiler without being added here.
