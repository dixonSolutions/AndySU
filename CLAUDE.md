# AndySU — agent notes

Read `docs/BRIEF.md` first. It holds the agreed design, the build order, the environment and the
rules. Keep `docs/PROGRESS.md` and `docs/DECISIONS.md` current.

- Pushing to `main` publishes a release. Get the step's tests passing locally before you push.
- A push whose head commit has `[skip ci]` skips CI for the whole push. Push docs-only commits
  separately.
- No git worktrees and no `isolation: "worktree"` agents: they have filled this disk before.
- `testbeds/` holds shallow clones of other projects. Never push to them, and never touch
  `~/Projects/Xonotic-Touch`.
- Never commit private keys. Every signing key in CI is a test key stored as an Actions secret.
- Don't create GitHub repositories; the owner does that.
