# UAT App Corpus

The regression corpus lives in `tests/corpus/apps/<case-id>`. Each case is a
source-only snapshot of a generated app plus `expectations.toml`.

Harvest a UAT workspace with:

```sh
mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704_003 \
  test0704_003
```

The script copies `src/**`, `package.json`, and common Next.js/TypeScript/
Tailwind/PostCSS config files. It intentionally does not copy `node_modules`,
`.next`, `.anvil`, lockfiles, logs, screenshots, or other generated artifacts.

After harvesting, edit `expectations.toml`:

- `required_paths`, `required_capabilities`, `required_evidence`, and
  `required_obligations` define the static acceptance contract for the case.
- `[route_closure]` pins files that must be included or excluded by the
  Next.js route-bound source closure.
- `[evidence]` pins detector tiers as `Strong`, `Weak`, or `Absent`.
- `[weak_evidence]` and `[diagnostics]` pin expected route-unbound or weak
  detector reasons.
- `[probe]` is optional. When `html_fixture` is present, the corpus test runs
  the static HTML version of the interaction probe hook/candidate selector.

Every future UAT false positive or false negative should add one case before
changing detector or probe logic.
