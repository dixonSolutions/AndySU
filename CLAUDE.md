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
- GitHub: on 2026-11-03 the owner authorised you to create the public repo `dixonSolutions/AndySU`.
  Don't create any other repository without asking; `andysu-remote-template` stays the owner's.
- Once the repo exists, work goes through PRs: branch, push, CI on the PR, review, then merge to
  `main` (that's the release). Don't push straight to `main` except for `[skip ci]` docs.

## Memory: this machine ran out of RAM on 2026-11-02 and the builder was killed

The machine has 62 GB, but other projects' agents and VMs use most of it, so check `free -g`
before heavy work.

- Run every heavy job (Gradle, emulators, cargo release builds, test-bed runs) inside
  `flock /tmp/andysu-heavy.lock systemd-run --user --scope -p MemoryMax=10G -- <cmd>`. That way
  only one runs at a time, and if a job goes over its limit, the job is killed, not your session
  or the desktop.
- Run one emulator at a time. Gradle uses `-Xmx2g`, plus `./gradlew --stop` when a run is done.
  Shut emulators down after each test.

## Workers and review

You are the lead (Opus). Hand self-contained tasks to workers through the Agent tool:

- `model: "sonnet"` for implementation;
- `model: "haiku"` for mechanical work (renames, docs, fixtures, boilerplate, log digging).

Brief each worker with the exact files, the acceptance test and these rules. Workers share this
checkout, so give parallel workers parts of the tree that don't overlap. Heavy jobs follow the
lock rule above.

Review every worker's result yourself before it is committed: read the diff, run the acceptance
test, and check it against the brief and the spec. Fix the result, or send it back, if it falls
short. Note in `docs/PROGRESS.md` which work came from a worker and how you verified it.
