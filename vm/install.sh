#!/usr/bin/env bash
set -euo pipefail

SCRIPT_NAME="install.sh"
PROJECT_ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN_NAME="vm"
CARGO_BIN_DIR="$HOME/.cargo/bin"

printf '%s: installing %s crate...\n' "$SCRIPT_NAME" "$BIN_NAME"
(cd "$PROJECT_ROOT" && cargo install --path . --force)

if ! [ -x "$CARGO_BIN_DIR/$BIN_NAME" ]; then
  printf 'Error: %s binary was not found in %s.\n' "$BIN_NAME" "$CARGO_BIN_DIR" >&2
  exit 1
fi

ensure_path_entry() {
  local target_file="$1"
  if [ -z "$target_file" ]; then
    return 0
  fi

  if [ -f "$target_file" ] && grep -q '\.cargo/bin' "$target_file"; then
    return 0
  fi

  {
    printf '\n# Added by %s\n' "$SCRIPT_NAME"
    printf '[ -d "$HOME/.cargo/bin" ] || mkdir -p "$HOME/.cargo/bin"\n'
    printf 'case ":$PATH:" in\n'
    printf '  *":$HOME/.cargo/bin:"*) ;;\n'
    printf '  *)\n'
    printf '    export PATH="$HOME/.cargo/bin:$PATH"\n'
    printf '    ;;\n'
    printf 'esac\n'
    printf '# End of %s\n' "$SCRIPT_NAME"
  } >> "$target_file"
}

if [ -n "${ZSH_VERSION:-}" ]; then
  ensure_path_entry "$HOME/.zshrc"
  ensure_path_entry "$HOME/.zprofile"
  SHELL_HINT="~/.zshrc and ~/.zprofile"
else
  ensure_path_entry "$HOME/.bashrc"
  ensure_path_entry "$HOME/.bash_profile"
  SHELL_HINT="~/.bashrc and ~/.bash_profile"
fi

printf 'Installation complete.\n'
printf 'Installed binary: %s\n' "$CARGO_BIN_DIR/$BIN_NAME"
printf 'Updated shell startup files: %s\n' "$SHELL_HINT"
printf 'Run: `source %s` (or open a new terminal) to refresh PATH.\n' \
  "$(if [ -n "${ZSH_VERSION:-}" ]; then printf '%s' '$HOME/.zshrc'; else printf '%s' '$HOME/.bashrc'; fi)"
printf 'Then: `%s --help`\n' "$BIN_NAME"
