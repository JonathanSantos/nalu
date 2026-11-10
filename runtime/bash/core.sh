# Nalu: Bash runtime, core (errors, commands, defer, ordinal text, durations).
# Each block starts with `# @fn name`; `# @needs` lists dependencies; `# @target linux|macos|all`.
# Every name starts with `__`. No `eval` on user data.

# @fn __err
# @target all
__err() { __err_msg=$1; __err_code=${2:-1}; return 1; }

# @fn __fail
# @target all
__fail() { printf 'error: %s\n' "$__err_msg" >&2; exit "${__err_code:-1}"; }

# @fn __usage_err
# @target all
__usage_err() { printf 'error: %s\nrun with --help to see the options\n' "$1" >&2; exit 2; }

# @fn __need
# @target all
__need() {
  local __h_p __h_falta=''
  for __h_p in "$@"; do
    type -P -- "$__h_p" >/dev/null 2>&1 || __h_falta+="${__h_falta:+, }$__h_p"
  done
  if [[ -n $__h_falta ]]; then
    printf 'error: this script needs: %s\n' "$__h_falta" >&2
    exit 127
  fi
}

# @fn __verb
# @target all
__verb() { if [[ $__verbose == true ]]; then printf '+ %s\n' "$1" >&2; fi; return 0; }

# @fn __show
# @target all
__show() {
  if [[ $__dry == true ]]; then printf '[dry-run] %s\n' "$1" >&2; return 1; fi
  if [[ $__verbose == true ]]; then printf '+ %s\n' "$1" >&2; fi
  return 0
}

# @fn __cmd_fail
# @needs __err
# @target all
__cmd_fail() {
  if [[ $2 == 127 ]]; then
    __err "command \`${3:-$1}\` was not found" 127
  else
    __err "command \`$1\` failed with exit code $2" "$2"
  fi
}

# @fn __cmd
# @needs __show __cmd_fail
# @target all
__cmd() {
  local __h_txt=$1 __h_rc
  shift
  __show "$__h_txt" || return 0
  "$@"
  __h_rc=$?
  (( __h_rc == 0 )) || __cmd_fail "$__h_txt" "$__h_rc" "$1"
}

# @fn __ok
# @needs __verb
# @target all
__ok() {
  __verb "$1"
  shift
  "$@"
}

# @fn __pipe_chk
# @needs __cmd_fail
# @target all
__pipe_chk() {
  local __h_txt=$1 __h_n=$2 __h_i __h_c __h_rc=0 __h_prog=''
  shift 2
  local -a __h_progs=("${@:1:__h_n}")
  shift "$__h_n"
  __h_i=0
  for __h_c in "$@"; do
    if (( __h_c != 0 )); then __h_rc=$__h_c; __h_prog=${__h_progs[__h_i]}; fi
    __h_i=$(( __h_i + 1 ))
  done
  (( __h_rc == 0 )) || __cmd_fail "$__h_txt" "$__h_rc" "$__h_prog"
}

# @fn __run_defers
# @target all
__run_defers() {
  local __ret __ret_a __err_msg __err_code __h_n __h_c
  while (( ${#__defers[@]} > $1 )); do
    __h_n=$(( ${#__defers[@]} - 1 ))
    __h_c=${__defers[__h_n]}
    local -a __h_w=("${__defers[@]:__h_n-__h_c:__h_c}")
    __defers=("${__defers[@]:0:__h_n-__h_c}")
    if ! "${__h_w[@]}"; then
      if [[ $__verbose == true ]]; then printf 'warning: defer failed: %s\n' "$__err_msg" >&2; fi
    fi
  done
  return 0
}

# @fn __str_lt
# @target all
__str_lt() { local LC_ALL=C; [[ $1 < $2 ]]; }

# @fn __sleep_ms
# @target all
__sleep_ms() {
  local __h_s
  (( $1 > 0 )) || return 0
  printf -v __h_s '%d.%03d' $(( $1 / 1000 )) $(( $1 % 1000 ))
  sleep "$__h_s"
}

# @fn __dur_str
# @target all
__dur_str() {
  local __h_ms=$1 __h_neg='' __h_out='' __h_v
  if (( __h_ms < 0 )); then __h_neg='-'; __h_ms=$(( -__h_ms )); fi
  if (( __h_ms == 0 )); then __ret='0s'; return 0; fi
  __h_v=$(( __h_ms / 86400000 )); (( __h_v > 0 )) && __h_out+="${__h_v}d"
  __h_v=$(( __h_ms / 3600000 % 24 )); (( __h_v > 0 )) && __h_out+="${__h_v}h"
  __h_v=$(( __h_ms / 60000 % 60 )); (( __h_v > 0 )) && __h_out+="${__h_v}m"
  __h_v=$(( __h_ms / 1000 % 60 ))
  local __h_r=$(( __h_ms % 1000 ))
  if (( __h_ms < 1000 )); then
    __h_out+="${__h_r}ms"
  elif (( __h_r > 0 )); then
    printf -v __h_r '%03d' "$__h_r"
    while [[ $__h_r == *0 ]]; do __h_r=${__h_r%0}; done
    __h_out+="${__h_v}.${__h_r}s"
  elif (( __h_v > 0 )); then
    __h_out+="${__h_v}s"
  fi
  __ret=$__h_neg$__h_out
}

# @fn __dur_parse
# @target all
__dur_parse() {
  # "1m30s", "1.5s", "250ms", "2h", "7d" → ms in __ret; status 1 if invalid.
  local __h_s=$1 __h_tot=0 __h_n __h_f __h_u __h_last=99
  local __h_neg=0
  [[ $__h_s == -* ]] && { __h_neg=1; __h_s=${__h_s#-}; }
  [[ -n $__h_s ]] || return 1
  while [[ -n $__h_s ]]; do
    __h_n=${__h_s%%[!0-9]*}
    [[ -n $__h_n ]] || return 1
    __h_s=${__h_s#"$__h_n"}
    __h_f=''
    if [[ $__h_s == .* ]]; then
      __h_s=${__h_s#.}
      __h_f=${__h_s%%[!0-9]*}
      [[ -n $__h_f ]] || return 1
      __h_s=${__h_s#"$__h_f"}
    fi
    case $__h_s in
      ms*) __h_u=1; __h_s=${__h_s#ms}; set -- 1 ;;
      s*) __h_u=1000; __h_s=${__h_s#s}; set -- 2 ;;
      m*) __h_u=60000; __h_s=${__h_s#m}; set -- 3 ;;
      h*) __h_u=3600000; __h_s=${__h_s#h}; set -- 4 ;;
      d*) __h_u=86400000; __h_s=${__h_s#d}; set -- 5 ;;
      *) return 1 ;;
    esac
    (( $1 < __h_last )) || return 1
    __h_last=$1
    [[ -z $__h_f || $__h_u == 1000 ]] || return 1
    (( ${#__h_n} <= 15 )) || return 1
    __h_tot=$(( __h_tot + 10#$__h_n * __h_u ))
    if [[ -n $__h_f ]]; then
      __h_f="${__h_f}000"
      __h_tot=$(( __h_tot + 10#${__h_f:0:3} ))
    fi
  done
  (( __h_neg )) && __h_tot=$(( -__h_tot ))
  __ret=$__h_tot
}

# @fn __int_parse
# @target all
__int_parse() {
  # 64-bit integer: digits only, optional `-`; result in __ret; status 1 if invalid
  local __h_s=${1#-} __h_m=9223372036854775807
  [[ -n $__h_s && $__h_s != *[!0-9]* ]] || return 1
  while [[ $__h_s == 0?* ]]; do __h_s=${__h_s#0}; done
  [[ $1 == -* ]] && __h_m=9223372036854775808
  (( ${#__h_s} < 19 )) || { (( ${#__h_s} == 19 )) && [[ ! $__h_s > $__h_m ]]; } || return 1
  if [[ $1 == -* && $__h_s != 0 ]]; then __ret=-$__h_s; else __ret=$__h_s; fi
}

# @fn __enum_parse
# @needs __err
# @target all
__enum_parse() {
  # __enum_parse Type text case1 case2 …
  local __h_t=$1 __h_v=$2 __h_c __h_l=''
  shift 2
  for __h_c in "$@"; do
    if [[ $__h_v == "$__h_c" ]]; then __ret=$__h_v; return 0; fi
    __h_l+="${__h_l:+, }$__h_c"
  done
  __err "'$__h_v' is not a valid $__h_t (use: $__h_l)"
}

# @fn __fmt
# @target all
__fmt() {
  local __h_t=$1 __h_o=''
  shift
  while [[ $__h_t == *'{}'* && $# -gt 0 ]]; do
    __h_o+="${__h_t%%'{}'*}$1"
    __h_t=${__h_t#*'{}'}
    shift
  done
  __ret=$__h_o$__h_t
}

# @fn __enc_a
# @target all
__enc_a() {
  __ret=''
  (( $# == 0 )) || printf -v __ret '%s\037' "$@"
}

# @fn __dec_a
# @target all
__dec_a() {
  local __h_s=$1
  __ret_a=()
  while [[ -n $__h_s ]]; do
    __ret_a+=("${__h_s%%$'\037'*}")
    __h_s=${__h_s#*$'\037'}
  done
}

# @fn __test_run
# @target all
__test_run() {
  # __test_run name function : runs a test in a subshell and prints `ok`/`FAILED`
  (
    __h_fim=0
    __h_nome=$1
    trap '__h_rc=$?; if (( ! __h_fim )); then printf "FAILED  %s: the test called exit(%s)\n" "$__h_nome" "$__h_rc"; exit 1; fi' EXIT
    if "$2"; then
      __h_fim=1
      printf 'ok      %s\n' "$1"
      exit 0
    fi
    __h_fim=1
    printf 'FAILED  %s: %s\n' "$1" "$__err_msg"
    exit 1
  )
}

# @fn __enc_l
# @target all
__enc_l() {
  # an inner list of a nested collection as one text: each item followed by \x1e (D34)
  __ret=''
  (( $# == 0 )) || printf -v __ret '%s\036' "$@"
}

# @fn __dec_l
# @target all
__dec_l() {
  local __h_s=$1
  __ret_a=()
  while [[ -n $__h_s ]]; do
    __ret_a+=("${__h_s%%$'\036'*}")
    __h_s=${__h_s#*$'\036'}
  done
}
