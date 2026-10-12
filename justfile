# One entry point: the git hook, CI and AGENTS.md all run `just check`.

shell_scripts := "assets/app-icon/render.sh packaging/linux/install.sh packaging/macos/bundle.sh .githooks/pre-commit"

# Everything CI gates on: format, lint, dependency policy, workflows, tests.
check: fmt-check lint deny workflows test

# Fast subset for the pre-commit hook (under a few seconds; no build).
pre-commit: fmt-check typos secrets-staged

fmt:
    cargo fmt
    shfmt -w {{ shell_scripts }}

fmt-check:
    cargo fmt --check
    shfmt -d {{ shell_scripts }}

lint: typos
    cargo clippy --locked --all-targets -- -D warnings
    shellcheck {{ shell_scripts }}

typos:
    typos

# RustSec advisories, licenses, banned and duplicate crates (deny.toml).
deny:
    cargo deny --locked check

# Workflow syntax (actionlint) and security (zizmor).
workflows:
    actionlint
    zizmor --offline .github/workflows

test:
    cargo test --locked

# Secrets in the whole git history (CI) and in the staged changes (hook).
secrets:
    gitleaks git --redact --no-banner

secrets-staged:
    gitleaks git --staged --redact --no-banner

# Install the pinned tools and the git hook.
setup:
    mise install
    git config core.hooksPath .githooks
