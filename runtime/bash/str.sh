# Nalu: Bash runtime, string methods. Everything per character (the prelude ensures a UTF-8 locale).

# @fn __str_trim
# @target all
__str_trim() {
  local __h_s=$1
  __h_s=${__h_s#"${__h_s%%[![:space:]]*}"}
  __ret=${__h_s%"${__h_s##*[![:space:]]}"}
}

# @fn __str_trim_start
# @target all
__str_trim_start() {
  __ret=${1#"${1%%[![:space:]]*}"}
}

# @fn __str_trim_end
# @target all
__str_trim_end() {
  __ret=${1%"${1##*[![:space:]]}"}
}

# @fn __str_upper
# @target linux
__str_upper() {
  __ret=${1^^}
}

# @fn __case_map
# @target macos
# Bash 3.2 has no ${x^^}/${x,,}, and one `tr` per call costs ~2.5 ms (a fork). Maps the letters of
# class $4 character by character through the ASCII + Latin-1 table ($2 → $3); if a letter of the class
# is outside the table (Greek, Cyrillic…), returns 1 and the caller uses `tr`, which gives the same result.
__case_map() {
  local __h_s=$1 __h_o='' __h_c __h_p __h_i
  for (( __h_i = 0; __h_i < ${#__h_s}; __h_i++ )); do
    __h_c=${__h_s:__h_i:1}
    case $__h_c in
      [[:$4:]])
        __h_p=${2%%"$__h_c"*}
        (( ${#__h_p} == ${#2} )) && return 1
        __h_c=${3:${#__h_p}:1} ;;
    esac
    __h_o+=$__h_c
  done
  __ret=$__h_o
}

# @fn __str_upper
# @needs __case_map
# @target macos
__str_upper() {
  if [[ $1 != *[[:lower:]]* ]]; then __ret=$1; return 0; fi
  if (( ${#1} <= 512 )) && __case_map "$1" 'abcdefghijklmnopqrstuvwxyzàáâãäåæçèéêëìíîïðñòóôõöøùúûüýþ' \
    'ABCDEFGHIJKLMNOPQRSTUVWXYZÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖØÙÚÛÜÝÞ' lower; then return 0; fi
  __ret=$(printf '%s.' "$1" | tr '[:lower:]' '[:upper:]')
  __ret=${__ret%.}
}

# @fn __str_lower
# @target linux
__str_lower() {
  __ret=${1,,}
}

# @fn __str_lower
# @needs __case_map
# @target macos
__str_lower() {
  if [[ $1 != *[[:upper:]]* ]]; then __ret=$1; return 0; fi
  if (( ${#1} <= 512 )) && __case_map "$1" 'ABCDEFGHIJKLMNOPQRSTUVWXYZÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖØÙÚÛÜÝÞ' \
    'abcdefghijklmnopqrstuvwxyzàáâãäåæçèéêëìíîïðñòóôõöøùúûüýþ' upper; then return 0; fi
  __ret=$(printf '%s.' "$1" | tr '[:upper:]' '[:lower:]')
  __ret=${__ret%.}
}

# @fn __str_index
# @target all
__str_index() {
  if [[ -z $2 ]]; then __ret=0; return 0; fi
  if [[ $1 != *"$2"* ]]; then __ret=-1; return 0; fi
  local __h_p=${1%%"$2"*}
  __ret=${#__h_p}
}

# @fn __str_slice
# @target all
__str_slice() {
  # like JS `slice`: negative counts from the end, out of range is clamped (D21)
  local __h_n=${#1} __h_a=$2 __h_b=$3
  if (( __h_a < 0 )); then __h_a=$(( __h_n + __h_a )); (( __h_a < 0 )) && __h_a=0; fi
  if (( __h_b < 0 )); then __h_b=$(( __h_n + __h_b )); (( __h_b < 0 )) && __h_b=0; fi
  (( __h_b > __h_n )) && __h_b=$__h_n
  if (( __h_a >= __h_b )); then __ret=''; return 0; fi
  __ret=${1:__h_a:__h_b-__h_a}
}

# @fn __str_split
# @target all
__str_split() {
  local __h_s=$1 __h_sep=$2 __h_i
  __ret_a=()
  if [[ -z $__h_sep ]]; then
    for (( __h_i = 0; __h_i < ${#__h_s}; __h_i++ )); do __ret_a+=("${__h_s:__h_i:1}"); done
    return 0
  fi
  while [[ $__h_s == *"$__h_sep"* ]]; do
    __ret_a+=("${__h_s%%"$__h_sep"*}")
    __h_s=${__h_s#*"$__h_sep"}
  done
  __ret_a+=("$__h_s")
}

# @fn __str_lines
# @target all
__str_lines() {
  local __h_s=$1 __h_l
  __ret_a=()
  while [[ -n $__h_s ]]; do
    if [[ $__h_s == *$'\n'* ]]; then
      __h_l=${__h_s%%$'\n'*}
      __h_s=${__h_s#*$'\n'}
    else
      __h_l=$__h_s
      __h_s=''
    fi
    __ret_a+=("${__h_l%$'\r'}")
  done
}

# @fn __str_replace
# @target all
__str_replace() {
  local __h_s=$1 __h_f=$2 __h_t=$3 __h_o=''
  if [[ -z $__h_f ]]; then __ret=$__h_s; return 0; fi
  while [[ $__h_s == *"$__h_f"* ]]; do
    __h_o+=${__h_s%%"$__h_f"*}$__h_t
    __h_s=${__h_s#*"$__h_f"}
  done
  __ret=$__h_o$__h_s
}

# @fn __str_replace_first
# @target all
__str_replace_first() {
  if [[ -z $2 || $1 != *"$2"* ]]; then __ret=$1; return 0; fi
  __ret=${1%%"$2"*}$3${1#*"$2"}
}

# @fn __str_repeat
# @target all
__str_repeat() {
  local __h_s=$1 __h_n=$2
  __ret=''
  while (( __h_n > 0 )); do
    if (( __h_n & 1 )); then __ret+=$__h_s; fi
    __h_s+=$__h_s
    __h_n=$(( __h_n >> 1 ))
  done
}

# @fn __str_pad
# @needs __str_repeat
# @target all
__str_pad() {
  # __str_pad left|right text n fill
  local __h_s=$2 __h_need=$(( $3 - ${#2} )) __h_p
  if (( __h_need <= 0 )) || [[ -z $4 ]]; then __ret=$__h_s; return 0; fi
  __str_repeat "$4" $(( __h_need / ${#4} + 1 ))
  __h_p=${__ret:0:__h_need}
  if [[ $1 == left ]]; then __ret=$__h_p$__h_s; else __ret=$__h_s$__h_p; fi
}

# @fn __str_to_int
# @needs __int_parse __str_trim __err
# @target all
__str_to_int() {
  __str_trim "$1"
  __int_parse "$__ret" || __err "'$1' is not an integer"
}

# @fn __str_chars
# @target all
__str_chars() {
  local __h_i
  __ret_a=()
  for (( __h_i = 0; __h_i < ${#1}; __h_i++ )); do __ret_a+=("${1:__h_i:1}"); done
}
