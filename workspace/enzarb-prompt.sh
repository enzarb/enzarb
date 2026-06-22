
# enzarb: custom prompt — sourced by /etc/bash.bashrc (interactive shells)
# and /etc/profile.d (login shells). Sets a clean hostname:path prompt and
# tells bash to track window size changes so the prompt redraws correctly
# after a terminal resize.
shopt -s checkwinsize
PS1='\[\e[1;32m\]\h\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]\$ '
