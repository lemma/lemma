# Vendored packages

## `vsce/`: `@vscode/vsce` 3.9.2

Copied from the published `@vscode/vsce@3.9.2` npm package (MIT, see `vsce/LICENSE`).
Changes, all in `vsce/package.json`: `secretlint` is removed from `dependencies`, and
`devDependencies`, `scripts` and the `mocha` config are removed (npm installs the dev
dependencies of a linked local package, and vsce needs none of them at runtime).

Why: `secretlint` pulls `globby` → `fast-glob` → `micromatch` → `picomatch`, and every
`picomatch` release is affected by GHSA-fc5q-39xc-vjpf (ReDoS), which fails
`npm audit --audit-level=low` in precommit. vsce never loads the `secretlint` CLI
package: `vsce/out/secretLint.js` imports only `@secretlint/node`, which remains a
dependency, so secret scanning during `vsce package` is unchanged. All other
dependencies still install from the registry and are audited.

`@vscode/vsce` 4.x is not an option: it depends on `tinyglobby`, which is built on
`picomatch`.

`../package.json` points `devDependencies."@vscode/vsce"` at `file:./vendor/vsce` and
overrides `@vscode/vsce` to the same, so `ovsx` uses this copy too.

Remove this vendored copy and return to the registry package once a `picomatch`
release fixes GHSA-fc5q-39xc-vjpf or a vsce release no longer depends on it.

Tracked in https://github.com/lemma/lemma/issues/113.
