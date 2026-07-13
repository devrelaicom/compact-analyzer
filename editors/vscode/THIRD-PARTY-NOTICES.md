# Third-party notices

The Compact Analyzer VS Code extension is licensed under the MIT License (see
`LICENSE`). It also incorporates and adapts third-party material, acknowledged
below.

## Compact editor support (LFDT-Minokawa/compact)

Portions of this extension — the TextMate grammar, the language configuration,
and the code snippets — are adapted from the official Compact VS Code editor
support in the `LFDT-Minokawa/compact` monorepo.

- **Project:** LFDT-Minokawa/compact
- **Upstream path:** `editor-support/vsc/compact/`
- **Pinned commit:** `fdfc61cf8b1311ca9fc5f8d155e1017d483a8acd`
- **Licence:** Apache License, Version 2.0
- **Licence text:** bundled with this extension as `LICENSE-Apache-2.0.txt`
  (a verbatim copy of the upstream `editor-support/vsc/compact/LICENSE` at the
  pinned commit), satisfying Apache-2.0 §4(a). The canonical text is also
  published at https://www.apache.org/licenses/LICENSE-2.0.

### Files adapted from upstream

| This extension | Upstream source | Change |
| --- | --- | --- |
| `language-configuration.json` | `editor-support/vsc/compact/language-configuration.json` | Copied verbatim. |
| `syntaxes/compact.tmLanguage.json` | `editor-support/vsc/compact/syntaxes/compact.tmLanguage.json` | Adapted: keyword/type alternations updated for Compact language 0.23; JavaScript-reserved-word pattern removed. |
| `snippets/compact.code-snippets` | `editor-support/vsc/compact/compact.code-snippets` | Adapted: snippet bodies audited and updated for Compact language 0.23. |
| `package.json` (`contributes.languages`) | `editor-support/vsc/compact/package.json` | Language `aliases` set to `["Compact", "compact"]` (upstream: `["compact"]`) so the display name appears first in the language picker. |

The upstream `scopeName` (`source.compact`) is preserved so existing colour
themes that target Compact continue to apply.

Upstream branding assets (the language file icons `logo-black.png` /
`logo-white.png`) are **not** included. Upstream problem matchers are **not**
carried; see `assets/NOTES.md` for the rationale.

The upstream `editor-support/vsc/compact/LICENSE` is the standard Apache-2.0
licence text with no project-specific copyright line and no accompanying
`NOTICE` file. Its verbatim copy ships as `LICENSE-Apache-2.0.txt` (Apache-2.0
§4(a)), and this notice — together with the change table above — provides the
attribution and the statement of modifications required by Apache-2.0 §4(b–d)
for the adapted material.

## Related distribution repository (not an asset source)

The Compact toolchain itself is distributed from a separate repository,
`midnightntwrk/compact` ("Compact Releases"), which contains no
`editor-support/` directory. It is the origin of toolchain install
documentation and CI used elsewhere in this project, and is recorded here only
to disambiguate it from the asset-source monorepo above.
