# Nalu: Bash runtime, maps.
#
# A map `m` lives in two dense arrays in insertion order, `m__k` (keys) and `m__v` (values),
# plus a key → position index:
# - linux (Bash 4.3+): `declare -A m__i` with the key prefixed by `_` (the empty key is valid);
# - macos (Bash 3.2, no associative arrays): one variable per key, `__m<id>_<encoded key>`,
#   read with `${!name}` and written with `printf -v`; `m__id` is the map's number and `m__n` holds the
#   names (to delete the index with a single `unset`). O(1) lookup: the cost is encoding the key (fast
#   for keys with only letters, digits, `_ - . /` and space; byte by byte for the rest).

# @fn __mkey
# @target macos
__mkey() {
  case $1 in
    *[!a-zA-Z0-9]*) ;;
    *) __mk=$1; return 0 ;;
  esac
  local __h_s=$1
  __h_s=${__h_s//_/_5f}
  __h_s=${__h_s//-/_2d}
  __h_s=${__h_s//./_2e}
  __h_s=${__h_s//\//_2f}
  __h_s=${__h_s// /_20}
  case $__h_s in
    *[!a-zA-Z0-9_]*) ;;
    *) __mk=$__h_s; return 0 ;;
  esac
  local LC_ALL=C __h_i __h_c __h_o=''
  __h_s=$1
  for (( __h_i = 0; __h_i < ${#__h_s}; __h_i++ )); do
    __h_c=${__h_s:__h_i:1}
    case $__h_c in
      [a-zA-Z0-9]) __h_o+=$__h_c ;;
      *) printf -v __h_c '%d' "'$__h_c"
         (( __h_c < 0 )) && __h_c=$(( __h_c + 256 ))
         printf -v __h_c '_%02x' "$__h_c"
         __h_o+=$__h_c ;;
    esac
  done
  __mk=$__h_o
}

# @fn __map_pos
# @needs __mkey
# @target macos
__map_pos() {
  # __map_pos id key → __mp (position or -1) and __mn (name of the index variable)
  __mk=$2
  case $__mk in
    *[!a-zA-Z0-9]*)
      __mk=${__mk//_/_5f}; __mk=${__mk//-/_2d}; __mk=${__mk//./_2e}; __mk=${__mk//\//_2f}; __mk=${__mk// /_20}
      case $__mk in
        *[!a-zA-Z0-9_]*) __mkey "$2" ;;
      esac ;;
  esac
  __mn=__m$1_$__mk
  __mp=${!__mn:--1}
}

# @fn __map_has
# @needs __map_pos
# @target macos
__map_has() {
  __map_pos "$1" "$2"
  (( __mp >= 0 ))
}

# @fn __map_put
# @needs __map_pos
# @target macos
__map_put() {
  # __map_put id key size: __mp = position; status 1 = new key (the caller stores k and n).
  __map_pos "$1" "$2"
  (( __mp >= 0 )) && return 0
  __mp=$3
  printf -v "$__mn" '%d' "$3"
  return 1
}

# @fn __map_index
# @needs __map_pos
# @target macos
__map_index() {
  # __map_index id : builds the index of the keys in __mi_k; the names go to __ret_a.
  # The keys come through a global array, not `$@`: in Bash 3.2 every function call copies the
  # positional arguments, and with 2,000 keys in `$@` the loop went quadratic (0.6 s → 0.04 s).
  local __h_id=$1 __h_i=0 __h_k
  __ret_a=()
  for __h_k in "${__mi_k[@]}"; do
    __map_pos "$__h_id" "$__h_k"
    __ret_a[__h_i]=$__mn
    printf -v "$__mn" '%d' "$__h_i"
    __h_i=$(( __h_i + 1 ))
  done
}

# @fn __map_del
# @needs __map_pos
# @target macos
__map_del() {
  # __map_del id key name… : status 0 if the key existed (__mp = position); renumbers the following ones.
  __map_pos "$1" "$2"
  (( __mp >= 0 )) || return 1
  unset "$__mn"
  shift 2
  local -a __h_ns=("$@")
  local __h_i
  for (( __h_i = __mp + 1; __h_i < ${#__h_ns[@]}; __h_i++ )); do
    printf -v "${__h_ns[__h_i]}" '%d' $(( __h_i - 1 ))
  done
  return 0
}

# @fn __map_err
# @needs __err
# @target all
__map_err() {
  __err "key '$1' does not exist"
}

# @fn __enc_m
# @target all
__enc_m() {
  # __enc_m n key… value… → "k\037v\037…" in __ret
  local __h_n=$1 __h_i
  shift
  __ret=''
  for (( __h_i = 1; __h_i <= __h_n; __h_i++ )); do
    __ret+="${!__h_i}"$'\037'
    __h_i=$(( __h_i + __h_n ))
    __ret+="${!__h_i}"$'\037'
    __h_i=$(( __h_i - __h_n ))
  done
}

# @fn __dec_m
# @target all
__dec_m() {
  # "k\037v\037…" → keys in __ret_a, values in __ret_b
  local __h_s=$1
  __ret_a=()
  __ret_b=()
  while [[ -n $__h_s ]]; do
    __ret_a+=("${__h_s%%$'\037'*}")
    __h_s=${__h_s#*$'\037'}
    __ret_b+=("${__h_s%%$'\037'*}")
    __h_s=${__h_s#*$'\037'}
  done
}

# @fn __map_reset
# @target macos
__map_reset() {
  # __map_reset id_var name… : deletes the old index; gives the map a new number if it has none yet
  local __h_v=$1
  shift
  (( $# == 0 )) || unset "$@"
  [[ -n ${!__h_v:-} ]] || printf -v "$__h_v" '%d' $(( ++__mid ))
}
