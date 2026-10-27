# Security policy

## Supported versions

Security fixes are made for the latest release. Please update to the newest version before
reporting a problem.

| Version | Supported |
|---|---|
| Latest release | Yes |
| Older releases | No |

## Reporting a vulnerability

Please do **not** report security problems in public issues, pull requests or discussions.

Report them privately through GitHub Security Advisories:

1. Go to the repository's **Security** tab.
2. Click **Report a vulnerability**.
3. Describe the problem, the affected version (`gitjet --version`), your OS and terminal, and the
   steps to reproduce it. A proof of concept helps.

You can also use this direct link: <https://github.com/OWNER/gitjet/security/advisories/new>.

We will confirm that we received the report, keep you updated while we investigate, and credit
you in the advisory when the fix is released, unless you prefer to stay anonymous. Please give us
reasonable time to release a fix before you disclose the problem publicly.

## What gitjet does with your credentials

- gitjet **never stores hosting tokens in its config file**, and the config file has no setting
  for them.
- Git operations run your installed `git`, so passwords and SSH keys are handled by Git, your
  credential helper and your SSH agent, exactly as on the command line. gitjet does not read or
  store them.
- Background Git runs with prompts disabled (`GIT_TERMINAL_PROMPT=0`, SSH in batch mode). When a
  password or passphrase is needed, the "(in terminal)" actions hand the terminal to Git, and you
  type it into Git directly.
- The pull request integration looks up API tokens from environment variables or Git's credential
  helper. See [docs/pull-requests.md](docs/pull-requests.md).

## Data gitjet writes

- Per repository: `.git/gitjet/` (changelists, shelf, local history snapshots of your files,
  favorites, commit message history). Local history contains copies of file contents, including
  files that are not committed. It stays inside the repository's `.git` folder and is never
  pushed.
- Globally: `config.toml` and `recent.json` in gitjet's config folder (`gitjet config-path`).

See [docs/configuration.md](docs/configuration.md#per-repository-data-gitgitjet) for the full
list.

## Things that are not vulnerabilities

- Commit checks (`[[commit.checks]]`) and the `editor` setting run commands from your own config
  file. That is their purpose.
- Git hooks in a repository run when gitjet runs Git, just as they do on the command line. Only
  open repositories you trust.
