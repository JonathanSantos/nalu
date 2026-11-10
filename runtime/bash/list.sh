# Nalu: Bash runtime, lists. Lists are dense arrays; the helpers take the items as arguments
# and return in __ret (scalar) or __ret_a (list). Sorting via `sort -z` (NUL: items with \n and \t
# work) with LC_ALL=C (ordinal order, by bytes = by code point in UTF-8).

# @fn __list_sort
# @target all
__list_sort() {
  # __list_sort str|int item…
  local __h_k=$1 __h_x
  shift
  __ret_a=()
  (( $# > 1 )) || { __ret_a=("$@"); return 0; }
  if [[ $__h_k == int ]]; then
    while IFS= read -r -d '' __h_x; do __ret_a+=("$__h_x"); done < <(printf '%s\0' "$@" | LC_ALL=C sort -z -n)
  else
    while IFS= read -r -d '' __h_x; do __ret_a+=("$__h_x"); done < <(printf '%s\0' "$@" | LC_ALL=C sort -z)
  fi
}

# @fn __list_perm
# @target all
__list_perm() {
  # __list_perm str|int key… → indices in stable key order (__ret_a)
  local __h_k=$1 __h_x __h_i
  shift
  __ret_a=()
  if (( $# < 2 )); then
    (( $# == 1 )) && __ret_a=(0)
    return 0
  fi
  local -a __h_r=()
  for (( __h_i = 1; __h_i <= $#; __h_i++ )); do __h_r+=("$(( __h_i - 1 ))"$'\t'"${!__h_i}"); done
  if [[ $__h_k == int ]]; then
    while IFS= read -r -d '' __h_x; do __ret_a+=("${__h_x%%$'\t'*}"); done < <(printf '%s\0' "${__h_r[@]}" | LC_ALL=C sort -z -s -t "$(printf '\t')" -k2,2n)
  else
    while IFS= read -r -d '' __h_x; do __ret_a+=("${__h_x%%$'\t'*}"); done < <(printf '%s\0' "${__h_r[@]}" | LC_ALL=C sort -z -s -t "$(printf '\t')" -k2)
  fi
}

# @fn __list_pick
# @target all
__list_pick() {
  # __list_pick n i1…in item… → the items at the requested positions (__ret_a)
  local __h_n=$1 __h_i
  shift
  local -a __h_ix=("${@:1:__h_n}")
  shift "$__h_n"
  local -a __h_it=("$@")
  __ret_a=()
  for __h_i in "${__h_ix[@]}"; do __ret_a+=("${__h_it[__h_i]}"); done
}

# @fn __list_join
# @target all
__list_join() {
  local __h_sep=$1 __h_x
  shift
  if (( ${#__h_sep} == 1 )); then
    local IFS=$__h_sep
    __ret="$*"
    return 0
  fi
  __ret=''
  (( $# )) || return 0
  __ret=$1
  shift
  for __h_x in "$@"; do __ret+=$__h_sep$__h_x; done
}

# @fn __list_index
# @target all
__list_index() {
  local __h_v=$1 __h_i=0 __h_x
  shift
  for __h_x in "$@"; do
    if [[ $__h_x == "$__h_v" ]]; then __ret=$__h_i; return 0; fi
    __h_i=$(( __h_i + 1 ))
  done
  __ret=-1
}

# @fn __list_has
# @target all
__list_has() {
  local __h_v=$1 __h_x
  shift
  for __h_x in "$@"; do
    [[ $__h_x == "$__h_v" ]] && return 0
  done
  return 1
}

# @fn __list_unique
# @needs __mkey
# @target macos
__list_unique() {
  # indices of the first occurrence of each item (__ret_a)
  local __h_i=0 __h_x __h_n
  __ret_a=()
  for __h_x in "$@"; do
    __mkey "$__h_x"
    __h_n=__u_$__mk
    if [[ -z ${!__h_n:-} ]]; then
      local "$__h_n=1"
      __ret_a+=("$__h_i")
    fi
    __h_i=$(( __h_i + 1 ))
  done
}

# @fn __list_unique
# @target linux
__list_unique() {
  local __h_i=0 __h_x
  local -A __h_seen=()
  __ret_a=()
  for __h_x in "$@"; do
    if [[ -z ${__h_seen["_$__h_x"]:-} ]]; then
      __h_seen["_$__h_x"]=1
      __ret_a+=("$__h_i")
    fi
    __h_i=$(( __h_i + 1 ))
  done
}

# @fn __list_range
# @target all
__list_range() {
  # __list_range start end size → __ra (start) and __rl (count), with Nalu's bounds
  # like JS `slice`: negative counts from the end, out of range is clamped (D21)
  local __h_a=$1 __h_b=$2 __h_n=$3
  if (( __h_a < 0 )); then __h_a=$(( __h_n + __h_a )); (( __h_a < 0 )) && __h_a=0; fi
  if (( __h_b < 0 )); then __h_b=$(( __h_n + __h_b )); (( __h_b < 0 )) && __h_b=0; fi
  (( __h_b > __h_n )) && __h_b=$__h_n
  (( __h_a > __h_n )) && __h_a=$__h_n
  __ra=$__h_a
  __rl=$(( __h_b > __h_a ? __h_b - __h_a : 0 ))
}

# @fn __list_reverse
# @target all
__list_reverse() {
  local __h_i
  __ret_a=()
  for (( __h_i = $#; __h_i > 0; __h_i-- )); do __ret_a+=("${!__h_i}"); done
}

# @fn __list_sum
# @target all
__list_sum() {
  local __h_x
  __ret=0
  for __h_x in "$@"; do __ret=$(( __ret + __h_x )); done
}

# @fn __list_empty_err
# @needs __err
# @target all
__list_empty_err() {
  __err "$1: the list is empty"
}
