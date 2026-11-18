# redraft

Browse a branch's history in the terminal and rewrite any commit in place, reword its message or edit its diff, with every later commit replayed on top. Comes with a CLI interface, and a TUI. 

## Why

I use `gitk` every day to look at git history. When cleaning up a branch for a PR I often resort to rebasing and changing the history then force pushing, but rebase conflicts are something I dont want to deal with, nor do I want to deal with having to type all these commands out in the terminal when there is already a way to view the history in `gitk`, its all there, but you cannot edit it. So this project is a minimal `gitk` clone that you can use to go on a branch, edit the diff by hand, and apply an interactive rebase with a replay strategy if needed for when the rebase is not clean.

## Quickstart

```bash
cargo install --path .
cd some/repo
redraft
```

That opens the commit list for the checked-out branch, with the selected commit's diff beside it. Press `e` to edit that diff in your editor, `r` to reword the message, and `?` for every key.

## How a rewrite works

- Only the first-parent chain from the edited commit to the branch tip is rebuilt. Authors, committers and dates are kept
- A reword keeps every tree as it was, so it can't conflict.
- An edited diff is applied to the commit's parent, then each later commit is cherry-picked onto the new one with a 3-way merge. On a conflict you pick per file: unify, yours, theirs, or edit by hand. You can also auto-resolve every file that merges cleanly.
- The branch only moves once the whole chain has replayed so aborting leaves it as it was.
- The old tip is saved under `refs/redraft-backup/<branch>-<time>`, so `git reset --hard <backup ref>` undoes a rewrite. The `H` key, or `redraft history`, lists them.
- If the branch is checked out, the working files are updated aswell, but only when none of the changed paths have uncommitted work.

## CLI Commands

The same rewrites work without the TUI. Output is JSON unless noted

| Command | What it does |
| --- | --- |
| `redraft log [-b BRANCH]` | Commits on the branch, newest first |
| `redraft show REV` | The commit's own diff, as plain text |
| `redraft reword REV -m MSG` | Replace the message (`-F FILE` or `-F -` reads it from a file or stdin) |
| `redraft edit REV --diff FILE [--resolve FILE] [--auto-resolve]` | Replace the commit's diff with an edited `show` output |
| `redraft history` | Past rewrites with their backup refs |

```bash
redraft show HEAD~3 > fix.diff
$EDITOR fix.diff
redraft edit HEAD~3 --diff fix.diff --auto-resolve
```

When replaying a later commit conflicts, `edit` exits with 3 and prints the conflicting files. Write the resolved text into a resolve file as `{"<commit sha>": {"<path>": "<text>"}}` and run the same command again with `--resolve`. Exit codes are 0 ok, 1 error, 2 bad usage, 3 conflict. `redraft --help` has the details, and `-C PATH` runs any command in another repository.

## Configuration

| Variable | Required | Purpose |
| --- | --- | --- |
| `GIT_EDITOR`, `core.editor`, `VISUAL`, `EDITOR` | No | Editor for messages and diffs, picked in git's order |

`o` opens a commit with `git show`, so it uses git's own pager settings.

## Development

| Command | What it does |
| --- | --- |
| `just run [repo]` | Run the TUI from source against a repo |
| `just test` | `cargo test`, then `cargo clippy --all-targets` |
| `just build` | Release build |
| `just install` | `cargo install --path .` |

## License

MIT
