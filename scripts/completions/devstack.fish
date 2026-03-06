# fish completion for devstack

function __devstack_complete
    set -l tokens (commandline -opc)
    set -l cur (commandline -ct)

    if test (count $tokens) -eq 0
        return
    end

    set -l words $tokens
    set -l cword

    if test "$cur" != ""
        if test "$words[-1]" = "$cur"
            set cword (math (count $words) - 1)
        else
            set cword (count $words)
            set words $words $cur
        end
    else
        set cword (count $words)
    end

    devstack __complete --cword $cword -- $words
end

complete -c devstack -f -a "(__devstack_complete)"
