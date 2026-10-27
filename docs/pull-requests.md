[← Back to docs index](README.md)

# Pull requests

Tab **7** lists and manages pull requests (merge requests on GitLab) without leaving the
terminal. It is not tied to one provider: gitjet talks to each provider's web API
directly and needs no extra command-line tool. Diffs and checkouts use plain `git`.

## Supported providers

| Provider | Detected from | Pull request ref used for diff/checkout |
|---|---|---|
| GitHub and GitHub Enterprise | `github.com`, host names containing "github", or a server whose `/api/v3/meta` answers | `refs/pull/N/head` |
| GitLab (gitlab.com and self-hosted) | `gitlab.com` and well-known public instances, host names containing "gitlab", or `/api/v4/version` | `refs/merge-requests/N/head` |
| Bitbucket Cloud | `bitbucket.org` | source branch, fetched from the source repository |
| Bitbucket Server / Data Center | host names containing "bitbucket" or "stash", or `/rest/api/1.0/application-properties` | `refs/pull-requests/N/from` |
| Azure DevOps | `dev.azure.com`, `ssh.dev.azure.com`, `*.visualstudio.com`, host names containing "azure" or "tfs" | `refs/pull/N/merge` |
| Gitea, Forgejo, Codeberg | `codeberg.org`, `gitea.com`, `next.forgejo.org`, host names containing "gitea", "forgejo" or "codeberg", or `/api/v1/version` | `refs/pull/N/head` |
| Anything else | — | browser links only (see below) |

Detection looks at every remote, tries config overrides first, then known hosts, host-name hints,
and finally probes unknown servers in the background. The result is cached in `.git/gitjet/`.

### What each provider can do

| | List and details | Create | Comment | Approve / request changes | Merge | Close / reopen |
|---|---|---|---|---|---|---|
| GitHub | yes (reviews, checks, files) | yes, incl. forks and drafts | yes | yes / yes | merge, squash, rebase | yes / yes |
| GitLab | yes (notes, approvals, pipeline) | yes, drafts as "Draft:" | yes | yes / withdraws approval and comments | merge, squash | yes / yes |
| Bitbucket Cloud | yes (participants, statuses, diffstat) | yes | yes | yes / yes | merge, squash, fast-forward | decline / not supported by the API |
| Bitbucket Server | yes (reviewers, builds, merge checks) | yes | yes | yes / "needs work" | merge, squash, rebase | yes / yes |
| Azure DevOps | yes (votes, threads, statuses) | yes, not from forks | yes | yes / "waiting for author" | merge, squash, rebase | abandon / reactivate |
| Gitea / Forgejo | yes (reviews, statuses, files) | yes, drafts as "WIP:" | yes | yes / yes | merge, squash, rebase | yes / yes |

Merging can also delete the source branch. When a provider can't do something, gitjet shows an
error explaining why instead of hiding the action.

## Signing in

Public repositories can be browsed without signing in (the header says *read-only*). For
anything else gitjet needs a token. It never stores tokens in its own config. It looks, in order:

1. `GITJET_TOKEN_<HOST>`: the host upper-cased with other characters replaced by `_`,
   e.g. `GITJET_TOKEN_GIT_CORP_COM_8443` for `git.corp.com:8443`
2. `GITJET_TOKEN`
3. the provider's usual variables:

   | Provider | Variables |
   |---|---|
   | GitHub | `GITHUB_TOKEN`, `GH_TOKEN` (Enterprise: `GITHUB_ENTERPRISE_TOKEN`, `GH_ENTERPRISE_TOKEN` first) |
   | GitLab | `GITLAB_TOKEN`, `GITLAB_PRIVATE_TOKEN` |
   | Bitbucket Cloud | `BITBUCKET_TOKEN`, or `BITBUCKET_USERNAME` + `BITBUCKET_APP_PASSWORD` |
   | Bitbucket Server | `BITBUCKET_SERVER_TOKEN`, `BITBUCKET_TOKEN` |
   | Azure DevOps | `AZURE_DEVOPS_EXT_PAT`, `AZURE_DEVOPS_TOKEN`, `SYSTEM_ACCESSTOKEN` |
   | Gitea / Forgejo | `GITEA_TOKEN`, `FORGEJO_TOKEN`, `CODEBERG_TOKEN` |

4. your Git credential helper (Git Credential Manager, macOS Keychain, libsecret, …), the
   same store `git push` uses. Turn this off with `use_git_credentials = false` under
   `[hosting]`.

**Sign in from the app:** press `L`. Paste a personal access token (Bitbucket also asks for your
username). gitjet checks it against the provider and saves it with `git credential approve`, so it
goes into your operating system's credential store. This needs a credential helper; if none is
configured, gitjet tells you how to set one up.

## Self-hosted servers

Most servers are recognised automatically. If yours isn't, tell gitjet what it is in the
[config file](configuration.md):

```toml
[[hosting.hosts]]
host = "git.example.com"
kind = "gitlab"     # github, gitlab, bitbucket, bitbucket-server, azure, gitea, none
# api_url = "https://git.example.com/gitlab/api/v4"   # only when the API isn't at the default place
```

`kind = "none"` turns the integration off for that host.

## Using the tab

The header shows the provider, host and repository, and whether you're signed in. The left pane
lists pull requests; the right pane shows the selected one: state, branches, +/- counts, checks,
reviews, description and comments.

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | move / scroll |
| `Tab` | switch between list and details |
| `PgUp` `PgDn` / `Space` | page |
| `g` / `Home`, `End` | first / last |
| `/` | search the loaded pull requests |
| `s` | cycle the state filter: open, closed, merged, all |
| `Enter` / `d` | show the diff in the [diff viewer](diff-viewer.md) |
| `c` | check out as local branch `pr/N` |
| `n` | create a pull request (title, description, base branch, draft) |
| `N` | create it in the browser |
| `C` | comment |
| `r` | review: approve, request changes or comment |
| `G` | merge (method, delete branch) |
| `x` | close / reopen |
| `w` | open the pull request in the browser |
| `W` / `B` | open the repository / current branch in the browser |
| `L` | sign in |
| `R` | reload |
| `.` / `m` | all actions |
| `Esc` | clear the search / back |

Creating a pull request pushes your branch first if it has no upstream yet.

### Diff and checkout work everywhere

`Enter` and `c` fetch the provider's pull request ref (see the first table) with `git fetch` and
then use ordinary Git: the diff is `git diff base...pr/N`, and checkout switches to `pr/N`. No API
call is needed for this, so it also works when you aren't signed in, as long as `git fetch` can
reach the remote.

## Hosts without an API

For any other host, or when the API can't be reached, the tab shows what it detected and what you
can still do:

| Key | Action |
|---|---|
| `W` | open the repository in the browser |
| `B` | open the current branch in the browser |
| `H` | open the HEAD commit in the browser |
| `N` | create a pull request in the browser (when the host's URL pattern is known) |
| `c` / `d` | check out / diff a pull request by number (when the provider has a pull request ref) |
| `L` | sign in |
| `R` | check again |

Every other gitjet feature works the same with any remote, including no remote at all.

## Action palette

`:` then type: **Create pull request…**, **View pull requests**, **Checkout pull request…**,
**Open repository in browser**, **Sign in to hosting provider…**.
