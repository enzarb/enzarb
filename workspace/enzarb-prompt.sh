
# enzarb: custom prompt — sourced by /etc/bash.bashrc (interactive shells)
# and /etc/profile.d (login shells). Sets a clean hostname:path prompt and
# tells bash to track window size changes so the prompt redraws correctly
# after a terminal resize.
shopt -s checkwinsize
PS1='\[\e[1;32m\]\h\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]\$ '

# enzarb: activate mise in interactive shells so `cd`-ing between project
# directories re-resolves tool versions and applies each mise.toml's [env]
# vars (shims-on-PATH already covers non-interactive/agent processes).
if command -v mise >/dev/null 2>&1; then
    eval "$(mise activate bash)"
fi
