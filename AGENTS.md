# Repository Rules

- Before writing any temporary artifact or any file that may contain local filesystem paths or other sensitive information, verify that the destination is effectively ignored by Git (`git check-ignore`) and is not tracked (`git ls-files`). Keep these files untracked; never bypass ignore rules with `git add -f`.
- Place all one-off validation, troubleshooting, benchmark, and experiment artifacts in the existing repository-root `tmp/` directory. This includes temporary code, input copies, outputs, logs, screenshots, and reports, regardless of whether they contain sensitive information. Do not create or copy them into committable project directories, including for delivery.
- Move a temporary tool or test into a permanent project directory only when the user explicitly requests retaining it as a project asset, and remove local absolute paths and other sensitive information first.
