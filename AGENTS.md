## Principles

- When finished a feature or module, commit the code with conventional commit message.
- Do not include "Fix #N" or other auto-closing keywords in commit messages; close issues explicitly as described below.

## Closing issues

When an issue is implemented on `develop` and CI passes, leave a comment on it that names the commits and summarises what changed. Then:

- **Close it** if it is a bug, refactor, performance fix, docs or test-infrastructure change whose fix is reliably covered by an automated test or by CI (for example a regression test, a golden image, a fixture, or a kittest UI test). Say in the comment which test covers it. Also close investigation or design tickets whose deliverable is a document.
- **Leave it open with the `needs-qa` label** if it is a feature, a visual or layout change, a behaviour change users will feel, a quality judgement on real images (healing, selection, blend modes, imports of real files), or anything the tests can't see (a real desktop session, HiDPI, a Windows-only path). Add a short "To test" list to the comment. The user removes the label or closes the issue after checking.
- Work that lands on a PR instead of `develop` gets the `needs-review` label until the PR is merged.

When in doubt, prefer `needs-qa` over closing.
