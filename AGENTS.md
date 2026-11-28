# Rules for agents working on this fork

This is Occam (repository and files: `occam-rzr`), Aristotelesqc's fork of Secretarybird
(upstream: infinityInfinite/secbird), which stays its engine. It stands on its own: whether
upstream would accept a change doesn't matter. People see **Occam**; the repository, the
executables and the settings folder are **occam-rzr**; the crates keep upstream's names
(`secbird-core`, `secbird-tray`, `secbird`) so upstream merges stay simple.
Upstream's `CONTRIBUTING.md` is a guide worth reading, not a set of rules. Follow it when it
costs little, and when you depart from it, say so in the commit.

## Goal
Make the editor as pleasant to use as Synapse's customise screen, without any of Synapse's
weight: no account, no ads, no services, no telemetry, no heavy runtime. Keep the tool light,
stable and fast, and measure size, memory and startup when a change can affect them.

## Never
- **Real devices:** never touch the owner's real Razer devices or their real settings during
  development:
  - no writes to `%APPDATA%\occam-rzr` (nor the old `%APPDATA%\Secretarybird`);
  - no changes to the installed copy outside this repo;
  - never stop the tray that is running;
  - no full tray that can see real devices while it uses test profiles.
- **Razer material:** no Razer images, logos, mouse renders, colours (the green and black look)
  or fonts. Take ergonomics and workflow from Synapse, never its look.
- **Device writes that could brick a device:** no firmware or bootloader commands.
- **Upstream:** never contact upstream (issues, PRs, comments) unless the owner asks.

## How to test
- **Simulated devices:** `SECBIRD_SIM=mouse` (or `all`, or a list such as `mouse,keyboard`).
- **Hide real hardware:** add `SECBIRD_SIM_ONLY=1` so the tray sees no real device and takes its
  own lock. A test tray can then run beside the real one.
- **Isolated settings:** point `APPDATA` at a scratch folder, so test profiles never mix with
  real ones.
- **Editor only:** `occam-rzr-tray.exe --serve` serves just the editor and prints its URL. It has
  no tray and applies nothing at start.
- **Real devices:** test on them only when the owner asks. Back up `profiles.json` first
  (`occam-rzr export`, or copy it), make one change, check it, and put it back.
- **Before a commit, all of these must pass:**
  - `cargo test --workspace`;
  - `cargo clippy --all-targets -- -D warnings`;
  - `node --test crates/secbird-tray/tests/`.
- **Editor changes:** check light and dark themes, 1366x768, and `SECBIRD_SIM=all`. Nothing may
  overflow sideways.
- **Report honestly:** say what was tested, on what, and what wasn't.

Toolchain on Windows (PowerShell): `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"`, then
`cargo build --release`.

## Code and repo
- **English:** code, comments, identifiers, file and branch names, commit messages and the
  CHANGELOG.
- **French and other languages:** only in UI strings, through i18n. Every new editor string needs
  a French entry in `FR_MESSAGES` in `i18n.js`; a test enforces it.
- **Editor:** one self-contained HTML file with no build step. Use the colour tokens on `:root`,
  and respect `data-motion="reduce"` and `prefers-reduced-motion`.
- **Dependencies:** each new crate needs a reason in the commit message.
- **CHANGELOG:** every user-visible change gets a line under `[Unreleased]` in `CHANGELOG.md`,
  saying what the user sees and what it was tested on.
- **Branches and commits:** work on a feature branch, never on `main`, and make small commits.
  Commit and push only when the owner asks. Never rewrite pushed history.

## Delegating to Gemini (agy)
To hand work to Gemini through the Antigravity CLI, follow `.agents/skills/agy/SKILL.md`.
It covers the headless limits (no commands, so supply snapshots), the report in the response,
scoped authorization with no permission bypass, and verifying every result. Runs go in the
git-ignored `.agy-runs/`.

## Merging upstream
The fork's update check follows this repository (`repository` in `Cargo.toml`), never
upstream's releases. To take upstream's changes:
1. `git fetch upstream`.
2. On a new branch, `git merge upstream/main`.
3. Resolve conflicts, run all the checks above, and rebuild.

Then record the upstream version merged (see #32).

## Backlog
The backlog lives in this fork's GitHub issues, with labels `area: …` and `priority: next` /
`priority: later`. Large items have sub-issues. Reference the issue in commits (`#12`). Close
an issue when its work is pushed. Longer reports (audits, evaluations) may live in Markdown
files; link them from the issue.
