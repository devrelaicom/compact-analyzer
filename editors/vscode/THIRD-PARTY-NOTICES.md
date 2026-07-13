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

## Bundled runtime dependencies

The extension's sole runtime dependency, `vscode-languageclient`, together with
its transitive dependencies, is inlined by esbuild into the shipped
`dist/extension.js` (only the `vscode` host module is left external — see
`esbuild.mjs`). Because the distributed VSIX therefore contains copies of this
code, the licence and copyright notice of each bundled package is reproduced
below. The exact set was determined empirically from the `// node_modules/<pkg>`
provenance comments esbuild emits in the (unminified) built bundle, not from the
dependency manifest.

| Package | Version | Licence |
| --- | --- | --- |
| `vscode-languageclient` | 10.1.0 | MIT |
| `vscode-languageserver-protocol` | 3.18.2 | MIT |
| `vscode-languageserver-types` | 3.18.0 | MIT |
| `vscode-languageserver-textdocument` | 1.0.13 | MIT |
| `vscode-jsonrpc` | 9.0.1 | MIT |
| `balanced-match` | 4.0.4 | MIT |
| `brace-expansion` | 5.0.7 | MIT |
| `semver` | 7.8.5 | ISC |
| `minimatch` | 10.2.5 | BlueOak-1.0.0 |

### MIT Licence

`vscode-languageclient@10.1.0`, `vscode-languageserver-protocol@3.18.2`,
`vscode-languageserver-types@3.18.0`,
`vscode-languageserver-textdocument@1.0.13`, and `vscode-jsonrpc@9.0.1` are all
authored by Microsoft Corporation and carry the following notice verbatim (from
each package's bundled `License.txt`):

```text
Copyright (c) Microsoft Corporation

All rights reserved.

MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED *AS IS*, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

`balanced-match@4.0.4` and `brace-expansion@5.0.7` share the same two copyright
holders — Julian Gruber for the original code and Isaac Z. Schlueter for the
TypeScript port — and are licensed under the MIT Licence (from each package's
bundled `LICENSE`):

```text
MIT License

Copyright Julian Gruber <julian@juliangruber.com>

TypeScript port Copyright Isaac Z. Schlueter <i@izs.me>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### ISC Licence

`semver@7.8.5` is licensed under the ISC Licence (from the package's bundled
`LICENSE`):

```text
The ISC License

Copyright (c) Isaac Z. Schlueter and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR
IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### Blue Oak Model License 1.0.0

`minimatch@10.2.5` (authored by Isaac Z. Schlueter) is licensed under the Blue
Oak Model License 1.0.0. Its "Notices" clause requires that anyone who receives
a copy of the software also receives the text of the licence or a link to
<https://blueoakcouncil.org/license/1.0.0>; the full text is reproduced here
(from the package's bundled `LICENSE.md`):

```text
# Blue Oak Model License

Version 1.0.0

## Purpose

This license gives everyone as much permission to work with
this software as possible, while protecting contributors
from liability.

## Acceptance

In order to receive this license, you must agree to its
rules. The rules of this license are both obligations
under that agreement and conditions to your license.
You must not do anything with this software that triggers
a rule that you cannot or will not follow.

## Copyright

Each contributor licenses you to do everything with this
software that would otherwise infringe that contributor's
copyright in it.

## Notices

You must ensure that everyone who gets a copy of
any part of this software from you, with or without
changes, also gets the text of this license or a link to
<https://blueoakcouncil.org/license/1.0.0>.

## Excuse

If anyone notifies you in writing that you have not
complied with [Notices](#notices), you can keep your
license by taking all practical steps to comply within 30
days after the notice. If you do not do so, your license
ends immediately.

## Patent

Each contributor licenses you to do everything with this
software that would otherwise infringe any patent claims
they can license or become able to license.

## Reliability

No contributor can revoke this license.

## No Liability

**_As far as the law allows, this software comes as is,
without any warranty or condition, and no contributor
will be liable to anyone for any damages related to this
software or this license, under any kind of legal claim._**
```
