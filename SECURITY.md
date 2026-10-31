# Security

## Reporting a vulnerability

Please don't open a public issue. Use **Report a vulnerability** on the
repository's Security tab (GitHub private vulnerability reporting), and
include what you found, how to reproduce it and what it lets someone do.
You'll get an answer within a week.

## What's in scope

Houston runs on your Mac and holds credentials, so the interesting parts are:

- The local HTTP API and MCP server on `127.0.0.1` (`src/sources/local_api.rs`,
  `src/mcp.rs`): bearer token checks, the Origin check against DNS rebinding,
  request parsing.
- Secrets: the Keychain storage (`src/secrets.rs`) and the promise that
  secrets never reach `config.json`, Mission settings or API answers.
- AWS credentials: access keys go to the AWS CLI only through its
  environment, never its command line (`src/aws.rs`).
- Anything an island, a webhook payload or a release feed can make the
  overlay or Mission settings do (both are webviews).

## How the repository is kept honest

- Trivy (vulnerable dependencies, committed secrets, misconfigurations) on
  every pull request, every push to `main` and weekly.
- Dependabot for Cargo and GitHub Actions; actions are pinned to commit SHAs.
- `main` only changes through pull requests that pass CI.
