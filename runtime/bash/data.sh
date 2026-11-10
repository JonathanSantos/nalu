# Nalu: Bash runtime, data: json, hash and archive.

# ---- json -------------------------------------------------------------------------------------
# No jq or python: the script parses the JSON itself. One pass (cached per text) records, for each
# value, the path (keys and indices separated by \x1f; the root is ""), the type (s n b z o a: string,
# number, bool, null, object, list) and the span [start, end) in the text, in bytes. Reads return the
# span: the same text on all three targets (PowerShell does the same, without ConvertFrom-Json). All in
# LC_ALL=C: byte indices are O(1) and the JSON delimiters are ASCII, so they never split a character.

# @fn __json_ws
# @target all
__json_ws() {
  while (( __h_i < __h_n )); do
    case ${__h_s:__h_i:1} in
      ' ' | $'\t' | $'\n' | $'\r') __h_i=$(( __h_i + 1 )) ;;
      *) return 0 ;;
    esac
  done
  return 0
}

# @fn __json_str
# @target all
__json_str() {
  # __h_s[__h_i] holds a quote: advance past the closing quote
  __h_i=$(( __h_i + 1 ))
  while (( __h_i < __h_n )); do
    # skip 32-byte chunks with no quote or backslash (long strings)
    if [[ ${__h_s:__h_i:32} != *[\"\\]* ]] && (( __h_i + 32 <= __h_n )); then __h_i=$(( __h_i + 32 )); continue; fi
    case ${__h_s:__h_i:1} in
      '"') __h_i=$(( __h_i + 1 )); return 0 ;;
      '\') __h_i=$(( __h_i + 2 )) ;;
      *) __h_i=$(( __h_i + 1 )) ;;
    esac
  done
  return 1
}

# @fn __json_val
# @needs __json_ws __json_str __json_unescape
# @target all
__json_val() {
  # one value at __h_s[__h_i] with path $1 (recursive for objects and lists)
  local __h_k=${#__js_p[@]} __h_c __h_a __h_key __h_x=0
  __json_ws
  (( __h_i < __h_n )) || return 1
  __js_p[__h_k]=$1
  __js_a[__h_k]=$__h_i
  __h_c=${__h_s:__h_i:1}
  case $__h_c in
    '"')
      __js_t[__h_k]=s
      __json_str || return 1
      ;;
    '{')
      __js_t[__h_k]=o
      __h_i=$(( __h_i + 1 ))
      __json_ws
      if [[ ${__h_s:__h_i:1} == '}' ]]; then
        __h_i=$(( __h_i + 1 ))
      else
        while :; do
          __json_ws
          [[ ${__h_s:__h_i:1} == '"' ]] || return 1
          __h_a=$__h_i
          __json_str || return 1
          __json_unescape "${__h_s:__h_a+1:__h_i-__h_a-2}"
          __h_key=$__ret
          __json_ws
          [[ ${__h_s:__h_i:1} == ':' ]] || return 1
          __h_i=$(( __h_i + 1 ))
          __json_val "$1"$'\x1f'"$__h_key" || return 1
          __json_ws
          __h_c=${__h_s:__h_i:1}
          __h_i=$(( __h_i + 1 ))
          [[ $__h_c == ',' ]] && continue
          [[ $__h_c == '}' ]] && break
          return 1
        done
      fi
      ;;
    '[')
      __js_t[__h_k]=a
      __h_i=$(( __h_i + 1 ))
      __json_ws
      if [[ ${__h_s:__h_i:1} == ']' ]]; then
        __h_i=$(( __h_i + 1 ))
      else
        while :; do
          __json_val "$1"$'\x1f'"$__h_x" || return 1
          __h_x=$(( __h_x + 1 ))
          __json_ws
          __h_c=${__h_s:__h_i:1}
          __h_i=$(( __h_i + 1 ))
          [[ $__h_c == ',' ]] && continue
          [[ $__h_c == ']' ]] && break
          return 1
        done
      fi
      ;;
    *)
      __h_a=$__h_i
      while (( __h_i < __h_n )); do
        case ${__h_s:__h_i:1} in
          ',' | '}' | ']' | ' ' | $'\t' | $'\n' | $'\r') break ;;
        esac
        __h_i=$(( __h_i + 1 ))
      done
      __h_c=${__h_s:__h_a:__h_i-__h_a}
      case $__h_c in
        true | false) __js_t[__h_k]=b ;;
        null) __js_t[__h_k]=z ;;
        *)
          local __h_re='^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$'
          [[ $__h_c =~ $__h_re ]] || return 1
          __js_t[__h_k]=n
          ;;
      esac
      ;;
  esac
  __js_b[__h_k]=$__h_i
  return 0
}

# @fn __json_parse
# @needs __err __json_val __json_ws
# @target all
__json_parse() {
  if [[ -n ${__js_ok-} && $1 == "$__js_src" ]]; then return 0; fi
  local LC_ALL=C
  local __h_s=$1 __h_n=${#1} __h_i=0
  __js_ok=''
  __js_p=()
  __js_t=()
  __js_a=()
  __js_b=()
  if __json_val '' && __json_ws && (( __h_i == __h_n )); then
    __js_src=$1
    __js_ok=1
    return 0
  fi
  __err "json: invalid JSON"
}

# @fn __json_utf8
# @target all
__json_utf8() {
  # code point → UTF-8 bytes (no subshell; Bash 3.2 has no printf '\u')
  local __h_c=$1 __h_f
  if (( __h_c == 0 )); then __ret=''; return 0; fi
  if (( __h_c < 0x80 )); then
    printf -v __h_f '\\x%02x' "$__h_c"
  elif (( __h_c < 0x800 )); then
    printf -v __h_f '\\x%02x\\x%02x' $(( 0xC0 | __h_c >> 6 )) $(( 0x80 | (__h_c & 0x3F) ))
  elif (( __h_c < 0x10000 )); then
    printf -v __h_f '\\x%02x\\x%02x\\x%02x' $(( 0xE0 | __h_c >> 12 )) $(( 0x80 | (__h_c >> 6 & 0x3F) )) $(( 0x80 | (__h_c & 0x3F) ))
  else
    printf -v __h_f '\\x%02x\\x%02x\\x%02x\\x%02x' $(( 0xF0 | __h_c >> 18 )) $(( 0x80 | (__h_c >> 12 & 0x3F) )) $(( 0x80 | (__h_c >> 6 & 0x3F) )) $(( 0x80 | (__h_c & 0x3F) ))
  fi
  printf -v __ret "$__h_f"
}

# @fn __json_unescape
# @needs __json_utf8
# @target all
__json_unescape() {
  # the body of a JSON string (without the quotes) → text
  local __h_r=$1 __h_o='' __h_e __h_h __h_lo __h_cp
  local __h_hex='^[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]$'
  while [[ $__h_r == *\\* ]]; do
    __h_o+=${__h_r%%\\*}
    __h_r=${__h_r#*\\}
    __h_e=${__h_r:0:1}
    __h_r=${__h_r:1}
    case $__h_e in
      n) __h_o+=$'\n' ;;
      t) __h_o+=$'\t' ;;
      r) __h_o+=$'\r' ;;
      b) __h_o+=$'\b' ;;
      f) __h_o+=$'\f' ;;
      u)
        __h_h=${__h_r:0:4}
        if [[ $__h_h =~ $__h_hex ]]; then
          __h_r=${__h_r:4}
          __h_cp=$(( 16#$__h_h ))
          # surrogate pair: 👍 → one character
          if (( __h_cp >= 0xD800 && __h_cp <= 0xDBFF )) && [[ ${__h_r:0:2} == '\u' && ${__h_r:2:4} =~ $__h_hex ]]; then
            __h_lo=$(( 16#${__h_r:2:4} ))
            if (( __h_lo >= 0xDC00 && __h_lo <= 0xDFFF )); then
              __h_r=${__h_r:6}
              __h_cp=$(( 0x10000 + ((__h_cp - 0xD800) << 10) + (__h_lo - 0xDC00) ))
            fi
          fi
          __json_utf8 "$__h_cp"
          __h_o+=$__ret
        else
          __h_o+='\u'
        fi
        ;;
      *) __h_o+=$__h_e ;;
    esac
  done
  __ret=$__h_o$__h_r
}

# @fn __json_find
# @needs __err __json_parse
# @target all
__json_find() {
  # __json_find text path → __js_k (the value's index) or fails
  __json_parse "$1" || return 1
  local __h_p=$2 __h_w='' __h_k
  __h_p=${__h_p//\[/.}
  __h_p=${__h_p//\]/}
  __h_p=${__h_p#.}
  if [[ -n $__h_p ]]; then
    while [[ $__h_p == *.* ]]; do
      __h_w+=$'\x1f'${__h_p%%.*}
      __h_p=${__h_p#*.}
    done
    __h_w+=$'\x1f'$__h_p
  fi
  for (( __h_k = 0; __h_k < ${#__js_p[@]}; __h_k++ )); do
    if [[ ${__js_p[__h_k]} == "$__h_w" ]]; then
      __js_k=$__h_k
      return 0
    fi
  done
  __err "json: '$2' does not exist"
}

# @fn __json_item
# @needs __json_unescape
# @target all
__json_item() {
  # the value at index $1 as text: a string without quotes; anything else, the JSON span
  local LC_ALL=C
  __ret=${__js_src:__js_a[$1]:__js_b[$1]-__js_a[$1]}
  if [[ ${__js_t[$1]} == s ]]; then __json_unescape "${__ret:1:${#__ret}-2}"; fi
  return 0
}

# @fn __json_get
# @needs __err __json_find __json_item
# @target all
__json_get() {
  __json_find "$1" "$2" || return 1
  if [[ ${__js_t[__js_k]} == z ]]; then __err "json: '$2' is null"; return; fi
  __json_item "$__js_k"
}

# @fn __json_children
# @needs __err __json_find __json_item
# @target all
__json_children() {
  # __json_children text path type(a|o) what(item|chave) → __ret_a
  __json_find "$1" "$2" || return 1
  local __h_k=$__js_k __h_pre __h_x __h_rest
  if [[ ${__js_t[__h_k]} != "$3" ]]; then
    if [[ $3 == a ]]; then __err "json: '$2' is not a list"; else __err "json: '$2' is not an object"; fi
    return
  fi
  __h_pre=${__js_p[__h_k]}$'\x1f'
  __ret_a=()
  for (( __h_x = __h_k + 1; __h_x < ${#__js_p[@]} && __js_a[__h_x] < __js_b[__h_k]; __h_x++ )); do
    __h_rest=${__js_p[__h_x]#"$__h_pre"}
    [[ $__h_rest == *$'\x1f'* ]] && continue
    if [[ $4 == key ]]; then
      __ret_a+=("$__h_rest")
    else
      __json_item "$__h_x"
      __ret_a+=("$__ret")
    fi
  done
  return 0
}

# @fn __json_has
# @needs __json_find
# @target all
__json_has() {
  local __err_msg __err_code
  __json_find "$1" "$2" 2>/dev/null && [[ ${__js_t[__js_k]} != z ]]
}

# @fn __json_quote
# @target all
__json_quote() {
  local __h_s=$1 __h_bs='\' __h_q='"' __h_c __h_x __h_u
  __h_s=${__h_s//"$__h_bs"/"$__h_bs$__h_bs"}
  __h_s=${__h_s//"$__h_q"/"$__h_bs$__h_q"}
  __h_s=${__h_s//$'\n'/"\\n"}
  __h_s=${__h_s//$'\r'/"\\r"}
  __h_s=${__h_s//$'\t'/"\\t"}
  __h_s=${__h_s//$'\b'/"\\b"}
  __h_s=${__h_s//$'\f'/"\\f"}
  # the other (rare) control characters become \u00XX
  if [[ $__h_s == *[$'\x01'-$'\x1f']* ]]; then
    for (( __h_x = 1; __h_x < 32; __h_x++ )); do
      printf -v __h_c '\\x%02x' "$__h_x"
      printf -v __h_c "$__h_c"
      printf -v __h_u '\\u%04x' "$__h_x"
      __h_s=${__h_s//"$__h_c"/"$__h_u"}
    done
  fi
  __ret=\"$__h_s\"
}

# ---- hash -------------------------------------------------------------------------------------

# @fn __sha256_cmd
# @target all
__sha256_cmd() {
  # this machine's SHA-256 program: sha256sum (GNU, BusyBox), shasum (macOS) or openssl
  if type -P sha256sum >/dev/null 2>&1; then __ret='sha256sum'
  elif type -P shasum >/dev/null 2>&1; then __ret='shasum -a 256'
  elif type -P openssl >/dev/null 2>&1; then __ret='openssl dgst -sha256 -r'
  else __ret=''
  fi
}

# @fn __hash_sha256
# @needs __sha256_cmd
# @target all
__hash_sha256() {
  __sha256_cmd
  local __h_o
  __h_o=$(printf '%s' "$1" | $__ret)
  __ret=${__h_o%% *}
}

# @fn __hash_sha256_file
# @needs __err __tilde __sha256_cmd
# @target all
__hash_sha256_file() {
  __tilde "$1"
  if [[ ! -f $__p ]]; then __err "hash.sha256_file: '$1' does not exist"; return; fi
  local __h_f=$__p __h_o
  __sha256_cmd
  if [[ -z $__ret ]]; then __err "hash.sha256_file: found no sha256sum, shasum or openssl"; return; fi
  __h_o=$($__ret < "$__h_f") || { __err "hash.sha256_file: could not read '$1'"; return; }
  __ret=${__h_o%% *}
}

# ---- archive ----------------------------------------------------------------------------------

# @fn __archive_tar_create
# @needs __err __tilde __dry_skip
# @target all
__archive_tar_create() {
  __tilde "$1"
  local __h_f=$__p
  __tilde "$2"
  local __h_d=${__p%/}
  __dry_skip "archive.tar_create $1 $2" "$__h_f" && return 0
  [[ -d $__h_d ]] || { __err "archive.tar_create: folder '$2' does not exist"; return; }
  local __h_base=${__h_d##*/} __h_parent=${__h_d%/*}
  [[ $__h_parent == "$__h_d" ]] && __h_parent=.
  [[ -z $__h_parent ]] && __h_parent=/
  tar -czf "$__h_f" -C "$__h_parent" -- "$__h_base" 2>/dev/null || __err "archive.tar_create: could not create '$1'"
}

# @fn __archive_tar_extract
# @needs __err __tilde __dry_skip
# @target all
__archive_tar_extract() {
  __tilde "$1"
  local __h_f=$__p
  __tilde "$2"
  __dry_skip "archive.tar_extract $1 $2" "$__p" && return 0
  [[ -f $__h_f ]] || { __err "archive.tar_extract: '$1' does not exist"; return; }
  mkdir -p -- "$__p" && tar -xzf "$__h_f" -C "$__p" 2>/dev/null || __err "archive.tar_extract: could not extract '$1'"
}

# @fn __archive_zip_extract
# @needs __err __tilde __dry_skip
# @target all
__archive_zip_extract() {
  __tilde "$1"
  local __h_f=$__p
  __tilde "$2"
  __dry_skip "archive.zip_extract $1 $2" "$__p" && return 0
  [[ -f $__h_f ]] || { __err "archive.zip_extract: '$1' does not exist"; return; }
  mkdir -p -- "$__p" || { __err "archive.zip_extract: could not create '$2'"; return; }
  if type -P unzip >/dev/null 2>&1; then
    unzip -q -o "$__h_f" -d "$__p" >/dev/null 2>&1 && return 0
  elif tar --version 2>/dev/null | grep -q bsdtar; then
    # macOS tar (bsdtar) also reads zip
    tar -xf "$__h_f" -C "$__p" 2>/dev/null && return 0
  else
    __err "archive.zip_extract: needs unzip"
    return
  fi
  __err "archive.zip_extract: could not extract '$1'"
}

# ---- regex (D32): the compiler translated the pattern to ERE; the script's UTF-8 locale makes `.` one character ----

# @fn __re_test
# @target all
__re_test() {
  [[ $1 =~ $2 ]]
}

# @fn __re_find
# @needs __err
# @target all
__re_find() {
  # __re_find text ere pattern → __ret
  if [[ $1 =~ $2 ]]; then __ret=${BASH_REMATCH[0]}; return 0; fi
  __err "regex: no match for '$3'"
}

# @fn __re_groups
# @needs __err
# @target all
__re_groups() {
  # __re_groups text ere pattern → __ret_a (groups 1..n)
  if [[ $1 =~ $2 ]]; then __ret_a=("${BASH_REMATCH[@]:1}"); return 0; fi
  __err "regex: no match for '$3'"
}

# @fn __re_all
# @target all
__re_all() {
  # __re_all text ere anchor(0|s|e) → __ret_a. The pattern never matches empty text (checked by the compiler).
  # Without anchors, a match starts at the first occurrence of its text (a match there exists too).
  local __h_rest=$1 __h_m __h_pre
  __ret_a=()
  while [[ $__h_rest =~ $2 ]]; do
    __h_m=${BASH_REMATCH[0]}
    __ret_a+=("$__h_m")
    [[ $3 == 0 ]] || break
    __h_pre=${__h_rest%%"$__h_m"*}
    __h_rest=${__h_rest:$(( ${#__h_pre} + ${#__h_m} ))}
  done
}

# @fn __re_replace
# @target all
__re_replace() {
  # __re_replace text ere new anchor(0|s|e) → __ret. `new` is literal.
  local __h_rest=$1 __h_out='' __h_m __h_pre
  while [[ $__h_rest =~ $2 ]]; do
    __h_m=${BASH_REMATCH[0]}
    if [[ $4 == e ]]; then __h_pre=${__h_rest%"$__h_m"}; else __h_pre=${__h_rest%%"$__h_m"*}; fi
    __h_out+=$__h_pre$3
    __h_rest=${__h_rest:$(( ${#__h_pre} + ${#__h_m} ))}
    [[ $4 == 0 ]] || break
  done
  __ret=$__h_out$__h_rest
}

# ---- encoding ----------------------------------------------------------------------------------

# @fn __b64_enc
# @target all
__b64_enc() {
  __ret=$(printf '%s' "$1" | base64 | tr -d '\n')
}

# @fn __b64_d
# @target linux
__b64_d() { base64 -d; }

# @fn __b64_d
# @target macos
__b64_d() { base64 --decode; }

# @fn __b64_dec
# @needs __err __b64_d
# @target all
__b64_dec() {
  # spaces and line breaks ignored; URL-safe alphabet and missing padding accepted (same rules as Windows)
  local __h_s=$1 __h_d __h_f
  __h_s=${__h_s//[$' \t\r\n']/}
  __h_s=${__h_s//-/+}
  __h_s=${__h_s//_//}
  __h_d=${__h_s%=}
  __h_d=${__h_d%=}
  case $__h_d in
    *[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/]*) __err "encoding: the text is not Base64"; return ;;
  esac
  if (( ${#__h_d} % 4 == 1 )); then __err "encoding: the text is not Base64"; return; fi
  while (( ${#__h_d} % 4 != 0 )); do __h_d+='='; done
  __h_f=$(mktemp "${TMPDIR:-/tmp}/nalu.XXXXXX" 2>/dev/null) || { __err "encoding: could not create a temporary file"; return; }
  if ! printf '%s' "$__h_d" | __b64_d > "$__h_f" 2>/dev/null; then rm -f "$__h_f"; __err "encoding: the text is not Base64"; return; fi
  # text only: no NUL byte, valid UTF-8. The round trip is compared because musl's iconv (Alpine)
  # drops invalid bytes and still exits 0
  if ! LC_ALL=C tr -d '\000' < "$__h_f" | cmp -s - "$__h_f" \
    || { command -v iconv >/dev/null 2>&1 && ! iconv -f UTF-8 -t UTF-8 < "$__h_f" 2>/dev/null | cmp -s - "$__h_f"; }; then
    rm -f "$__h_f"
    __err "encoding: the decoded bytes are not UTF-8 text"
    return
  fi
  __ret=$(cat "$__h_f"; printf .)
  __ret=${__ret%.}
  rm -f "$__h_f"
}
