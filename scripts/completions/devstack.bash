# bash completion for devstack

_devstack_complete() {
  local cur
  cur="${COMP_WORDS[COMP_CWORD]}"
  local out
  out=$(devstack __complete --cword "${COMP_CWORD}" -- "${COMP_WORDS[@]}")
  COMPREPLY=()
  while IFS=$'\n' read -r line; do
    [[ -n "$line" ]] && COMPREPLY+=("$line")
  done <<< "$out"
}

complete -o bashdefault -o default -F _devstack_complete devstack
