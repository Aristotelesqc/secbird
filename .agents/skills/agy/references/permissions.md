# AGY permissions

Verified on Windows with Antigravity CLI 1.3.4 to 1.5.x. Check `agy --help` again when the
version differs.

## Two separate problems

1. **Permission.** A print (headless) run auto-denies tools that need confirmation, because no
   confirmation UI is attached. This is not a lost login or a network problem.
2. **Capability.** A tool that reads a URL's text can't click through an app. Check the tools
   that are actually exposed (`agy mcp list`) before assuming one exists.

An agent saying that access is granted is not evidence; require a real, successful tool result.
`[READ-TOKEN]` / `[WRITE-TOKEN]` are prompt conventions, not CLI settings (see `SKILL.md`).

## Interactive, scoped confirmation

When headless access is denied and the owner has authorized the access:

1. Resume the exact conversation in an attached terminal, so that real confirmation menus
   appear. Use the conversation ID from `run.json`, never the global most recent one:

   ```text
   agy --conversation <recorded-id> --model <model-from-run> --mode plan --log-file <private-log> --prompt-interactive <bounded-brief>
   ```

2. Inspect each pending tool, its arguments and its scope. Approve only the action already
   authorized, and prefer "allow in this conversation" to saving it in settings.
3. Never build an automatic approval loop.
4. Never use `--dangerously-skip-permissions`, allow-all settings or changes to global settings.
5. Confirm the real result of the approved tool before going on.

A session-only override is possible with `--settings <file>` (not saved), using anchored
`command(regex:...)` rules for one exact executable and argument pattern. Never use a generic
shell or interpreter grant. Keep such files in `.agy-runs/`.

## Simpler alternative: snapshot

When the delegate only needs data that a command produces (for example `gh api` output), the
caller runs the commands, saves the output as files in a corpus folder, and runs
`run_agy.py --snapshot --repo <corpus>` with a prompt that allows file read and search tools only.
Say in the report that it was a snapshot analysis, not direct access.
