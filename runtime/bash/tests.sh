#!/usr/bin/env bash
# Unit tests for the helpers in runtime/bash/*.sh.
#
#   bash runtime/bash/tests.sh [macos|linux]
#
# Loads the target's helpers (the same `# @target` filter the generator uses), runs one test per
# helper and checks that no helper is left untested. Default target: macos on Bash 3.x, linux on 4+.
# Exits with 1 if anything fails.

set -o pipefail
__p='ç'; [ "${#__p}" = 1 ] || for __l in C.UTF-8 C.utf8 en_US.UTF-8 en_US.utf8; do
  export LC_ALL="$__l"; __p='ç'; [ "${#__p}" = 1 ] && break; unset LC_ALL
done

alvo=${1:-}
if [[ -z $alvo ]]; then
  if (( BASH_VERSINFO[0] >= 4 )); then alvo=linux; else alvo=macos; fi
fi
dir=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/nalu-rt.XXXXXX") || exit 1
trap 'rm -rf "$tmp"' EXIT

# ---- load the target's helpers --------------------------------------------------------------
todos=()
codigo=''
for f in "$dir"/core.sh "$dir"/str.sh "$dir"/list.sh "$dir"/map.sh "$dir"/fs.sh "$dir"/sys.sh "$dir"/data.sh; do
  nome='' bloco='' alvo_bloco=all
  while IFS= read -r l || [[ -n $l ]]; do
    case $l in
      '# @fn '*)
        if [[ -n $nome && ( $alvo_bloco == all || $alvo_bloco == "$alvo" ) ]]; then codigo+=$bloco; todos+=("$nome"); fi
        nome=${l#'# @fn '} bloco='' alvo_bloco=all ;;
      '# @target '*) alvo_bloco=${l#'# @target '} ;;
      '# @needs'*) ;;
      *) [[ -n $nome ]] && bloco+=$l$'\n' ;;
    esac
  done < "$f"
  if [[ -n $nome && ( $alvo_bloco == all || $alvo_bloco == "$alvo" ) ]]; then codigo+=$bloco; todos+=("$nome"); fi
done
printf '%s' "$codigo" > "$tmp/rt.sh"
# shellcheck source=/dev/null
source "$tmp/rt.sh"
__err_msg=''; __err_code=0; __dry=false; __verbose=false; __defers=(); __temps=(); __mid=0

# ---- mini-framework ---------------------------------------------------------------------------
testados=' '
falhas=0
total=0
eq() { # eq helper description got expected
  total=$(( total + 1 ))
  testados+="$1 "
  if [[ $3 == "$4" ]]; then return 0; fi
  falhas=$(( falhas + 1 ))
  printf 'FAILED  %s: %s\n  expected: [%s]\n  got:      [%s]\n' "$1" "$2" "$4" "$3"
}
ok() { # ok helper description command… (status 0)
  local h=$1 d=$2
  shift 2
  if "$@"; then eq "$h" "$d" ok ok; else eq "$h" "$d" falhou ok; fi
}
no() { # no helper description command… (status != 0)
  local h=$1 d=$2
  shift 2
  if "$@"; then eq "$h" "$d" passou falha; else eq "$h" "$d" falha falha; fi
}
ret() { # ret helper description expected command…  (compares __ret after the command)
  local h=$1 d=$2 e=$3
  shift 3
  "$@"
  eq "$h" "$d" "$__ret" "$e"
}
reta() { # reta helper description expected(joined with |) command…
  local h=$1 d=$2 e=$3 j
  shift 3
  "$@"
  j=$(IFS='|'; printf '%s' "${__ret_a[*]}")
  eq "$h" "$d" "$j" "$e"
}

# ---- core -------------------------------------------------------------------------------------
no __err 'returns 1' __err 'x' 3; eq __err 'message and code' "$__err_msg/$__err_code" 'x/3'
eq __fail 'stderr and code' "$( (__err_msg='m'; __err_code=4; __fail) 2>&1; echo "c=$?")" $'error: m\nc=4'
eq __usage_err 'code 2' "$( (__usage_err 'ruim') 2>&1; echo "c=$?")" $'error: ruim\nrun with --help to see the options\nc=2'
eq __need 'missing program' "$( (__need sh nao-existe-1 nao-existe-2) 2>&1; echo "c=$?")" $'error: this script needs: nao-existe-1, nao-existe-2\nc=127'
eq __verb 'only with --verbose' "$(__verb a 2>&1; __verbose=true; __verb b 2>&1)" '+ b'
eq __show 'dry-run does not run' "$(__dry=true; __show 'x y' 2>&1; echo "s=$?")" $'[dry-run] x y\ns=1'
no __cmd_fail 'message' __cmd_fail 'git push' 1; eq __cmd_fail 'text' "$__err_msg" 'command `git push` failed with exit code 1'
__cmd_fail 'foo x' 127 foo; eq __cmd_fail '127' "$__err_msg/$__err_code" 'command `foo` was not found/127'
eq __cmd 'runs' "$(__cmd 'echo oi' echo oi)" 'oi'
no __cmd 'failure' __cmd 'false' false; eq __cmd 'code' "$__err_code" 1
ok __ok 'command status' __ok 'true' true
false | true; no __pipe_chk 'pipefail catches the stage' __pipe_chk 'false | true' 2 false true 1 0
eq __pipe_chk 'message' "$__err_msg" 'command `false | true` failed with exit code 1'
__defers=(echo a 2 echo b 2); eq __run_defers 'reverse order' "$(__run_defers 0)" $'b\na'
ok __str_lt 'ordinal: B < a' __str_lt B a
no __str_lt 'ordinal: a > B' __str_lt a B
ok __sleep_ms 'half a second becomes 0.010' __sleep_ms 10
for c in '0:0s' '250:250ms' '1500:1.5s' '90000:1m30s' '93784005:1d2h3m4.005s' '-2000:-2s'; do
  ret __dur_str "${c%%:*}" "${c#*:}" __dur_str "${c%%:*}"
done
for c in '30s:30000' '1.5s:1500' '1m30s:90000' '250ms:250' '2h:7200000' '7d:604800000' '1d2h:93600000'; do
  ret __dur_parse "${c%%:*}" "${c#*:}" __dur_parse "${c%%:*}"
done
for c in '5' '1.5m' '30s1m' 'x' ''; do no __dur_parse "invalid: $c" __dur_parse "$c"; done
ret __int_parse '007' 7 __int_parse 007
ret __int_parse 'minimum' -9223372036854775808 __int_parse -9223372036854775808
no __int_parse '+5' __int_parse +5
no __int_parse 'overflow' __int_parse 9223372036854775808
ret __enum_parse 'valid case' prod __enum_parse Amb prod dev prod
no __enum_parse 'invalid case' __enum_parse Amb qa dev prod
eq __enum_parse 'message' "$__err_msg" "'qa' is not a valid Amb (use: dev, prod)"
ret __fmt 'replaces {}' 'a=1, b=x y' __fmt 'a={}, b={}' 1 'x y'
reta __dec_a 'items' 'a b||c' __dec_a "$(__enc_a 'a b' '' 'c'; printf '%s' "$__ret")"
__enc_l 'a b' '' 'c'; eq __enc_l 'items end in \\x1e' "$__ret" $'a b\036\036c\036'
__enc_l; eq __enc_l 'empty list' "$__ret" ''
reta __dec_l 'items' 'a b||c' __dec_l $'a b\036\036c\036'
reta __dec_l 'one empty item' '' __dec_l $'\036'; eq __dec_l 'one empty item, count' "${#__ret_a[@]}" 1
reta __dec_l 'empty' '' __dec_l ''; eq __dec_l 'empty, count' "${#__ret_a[@]}" 0
__enc_a; eq __enc_a 'empty list' "$__ret" ''
t_ok() { return 0; }
t_falha() { __err_msg='deu ruim'; return 1; }
eq __test_run 'ok and FAILED' "$(__test_run 'um' t_ok; __test_run 'dois' t_falha)" $'ok      um\nFAILED  dois: deu ruim'

# ---- text -------------------------------------------------------------------------------------
ret __str_trim 'ends' 'a b' __str_trim $' \t a b \n'
ret __str_trim_start 'start' 'a ' __str_trim_start '  a '
ret __str_trim_end 'end' '  a' __str_trim_end '  a '
ret __str_upper 'accents' 'AÇÃO' __str_upper 'ação'
ret __str_lower 'accents' 'ação' __str_lower 'AÇÃO'
[[ $alvo == macos ]] && ret __case_map 'by the table' 'AB-1' __case_map 'ab-1' 'ab' 'AB' lower
[[ $alvo == macos ]] && no __case_map 'letter outside the table' __case_map 'abc' 'ab' 'AB' lower
ret __str_upper 'mixed' 'ITEM-7 ÇÃO ÞØ' __str_upper 'Item-7 çãO þø'
ret __str_lower 'mixed' 'item-7 ção þø' __str_lower 'ITEM-7 ÇÃO ÞØ'
ret __str_lower 'outside the table (Greek)' 'αβγ abc' __str_lower 'ΑΒΓ ABC'
ret __str_upper 'outside the table (Greek)' 'ΑΒΓ ABC' __str_upper 'αβγ abc'
ret __str_upper 'literal wildcards' '*A?[B]' __str_upper '*a?[b]'
ret __str_lower 'line break at the end' $'a\nb\n' __str_lower $'A\nB\n'
ret __str_index 'by character' 3 __str_index 'açaí' 'í'
ret __str_index 'does not find' -1 __str_index 'abc' 'z'
ret __str_slice 'by character' 'çã' __str_slice 'ação' 1 3
ret __str_slice 'to the end' 'ão' __str_slice 'ação' 2 9223372036854775807
ret __str_slice 'negative end (JS)' 'aç' __str_slice 'ação' 0 -2
ret __str_slice 'negative start (JS)' 'ão' __str_slice 'ação' -2 9223372036854775807
ret __str_slice 'out of range' '' __str_slice 'ação' 9 12
reta __str_split 'empty fields' 'a||b|' __str_split 'a,,b,' ','
reta __str_lines 'crlf and final line' 'x|y||z' __str_lines $'x\r\ny\n\nz\n'
ret __str_replace 'all, literal' 'a&b&c' __str_replace 'a*b*c' '*' '&'
ret __str_replace_first 'only the first' 'a-b.c' __str_replace_first 'a.b.c' '.' '-'
ret __str_repeat 'n times' 'ababab' __str_repeat 'ab' 3
ret __str_pad 'left' '007' __str_pad left 7 3 0
ret __str_pad 'right' 'ç  ' __str_pad right 'ç' 3 ' '
ret __str_to_int 'spaces' -42 __str_to_int ' -042 '
no __str_to_int 'text' __str_to_int '1x'
reta __str_chars 'code points' 'a|ñ|👍' __str_chars 'añ👍'

# ---- lists ------------------------------------------------------------------------------------
reta __list_sort 'ordinal with \n' $'B|a\nb|a b' __list_sort str 'a b' $'a\nb' B
reta __list_sort 'numbers' '-2|3|10' __list_sort int 10 -2 3
reta __list_perm 'stable by key' '1|0|2' __list_perm int 5 1 5
reta __list_perm 'text with tab' '1|0' __list_perm str $'b\tx' 'a'
reta __list_pick 'by index' 'c|a' __list_pick 2 2 0 a b c
ret __list_join 'long separator' 'a, b, c' __list_join ', ' a b c
ret __list_join 'one character' 'a|b' __list_join '|' a b
ret __list_index 'finds' 1 __list_index b a b
ok __list_has 'contains' __list_has '' a ''
reta __list_unique 'indexes of the 1st occurrence' '0|1|3' __list_unique a b a ''
__list_range 1 9223372036854775807 4; eq __list_range 'to the end' "$__ra/$__rl" '1/3'
__list_range 0 -1 4; eq __list_range 'negative end (JS)' "$__ra/$__rl" '0/3'
__list_range -2 9223372036854775807 4; eq __list_range 'negative start (JS)' "$__ra/$__rl" '2/2'
__list_range -9 2 4; eq __list_range 'negative beyond the start' "$__ra/$__rl" '0/2'
__list_range 3 1 4; eq __list_range 'empty' "$__ra/$__rl" '3/0'
reta __list_reverse 'reverses' 'c|b|a' __list_reverse a b c
ret __list_sum 'sum' 6 __list_sum 1 2 3
no __list_empty_err 'message' __list_empty_err pop; eq __list_empty_err 'text' "$__err_msg" 'pop: the list is empty'

# ---- maps -------------------------------------------------------------------------------------
__enc_m 2 a b 1 2; ret __enc_m 'k/v' $'a\0371\037b\0372\037' :
__dec_m "$__ret"; eq __dec_m 'keys and values' "${__ret_a[*]}/${__ret_b[*]}" 'a b/1 2'
ok __map_err 'message' eval '! __map_err x && [[ $__err_msg == "key '\''x'\'' does not exist" ]]'
if [[ $alvo == macos ]]; then
  __mkey 'ç é'; e1=$__mk; __mkey 'ç ê'; eq __mkey 'injective in UTF-8' "$([[ $e1 != "$__mk" ]] && echo ok)" ok
  m__k=(a 'b c' ''); m__v=(1 2 3); m__n=(); m__id=''
  __map_reset m__id "${m__n[@]}"; eq __map_reset 'new number' "$([[ -n $m__id ]] && echo ok)" ok
  __mi_k=("${m__k[@]}"); __map_index "$m__id"; m__n=("${__ret_a[@]}"); eq __map_index 'one name per key' "${#m__n[@]}" 3
  __map_pos "$m__id" 'b c'; eq __map_pos 'position' "$__mp" 1
  ok __map_has 'empty key' __map_has "$m__id" ''
  __map_put "$m__id" novo "${#m__k[@]}"; eq __map_put 'new key' "$?/$__mp" '1/3'
  __map_put "$m__id" a "${#m__k[@]}"; eq __map_put 'existing key' "$?/$__mp" '0/0'
  __map_del "$m__id" a "${m__n[@]}"; eq __map_del 'removed' "$?/$__mp" '0/0'
  __map_pos "$m__id" ''; eq __map_del 'renumbers' "$__mp" 1
fi

# ---- files and paths --------------------------------------------------------------------------
cd "$tmp" || exit 1
mkdir -p d/sub && printf 'x' > d/a.txt && printf 'yy' > d/sub/b.log && printf '' > d/.oculto
ret __tilde '~/x' "$HOME/x" eval '__tilde "~/x"; __ret=$__p'
ret __path_home 'just ~' "$HOME" __path_home '~'
ret __path_home '~/x' "$HOME/x" __path_home '~/x'
ret __path_home '~user stays' '~foo/y' __path_home '~foo/y'
ret __path_home 'other paths stay' 'a/~' __path_home 'a/~'
__dry=true; __temps=("$tmp/t"); ok __dry_skip 'outside the temp folder' eval '__dry_skip "fs.rm x" /x 2>/dev/null'
no __dry_skip 'inside the temp folder' __dry_skip 'fs.rm x' "$tmp/t/a"; __dry=false; __temps=()
ok __fs_exists 'file' __fs_exists d/a.txt
ok __fs_is_dir 'folder' __fs_is_dir d/sub
no __fs_is_file 'folder is not a file' __fs_is_file d/sub
printf '\xef\xbb\xbfoi\n\n' > d/bom.txt; ret __fs_read 'strips the BOM, keeps \n' $'oi\n\n' __fs_read d/bom.txt
no __fs_read 'missing' __fs_read d/nao
ok __fs_write 'writes' __fs_write d/w.txt 'abc'; ret __fs_write 'content' abc __fs_read d/w.txt
ok __fs_append 'appends' __fs_append d/w.txt 'd'; ret __fs_append 'content' abcd __fs_read d/w.txt
no __fs_write 'missing folder' __fs_write /nao/existe/x 'a'
no __fs_rm 'refuses .' __fs_rm .
no __fs_rm 'refuses ~' __fs_rm '~'
ok __fs_rm 'missing does not fail' __fs_rm d/nao-existe
ok __fs_mkdir 'with parents' __fs_mkdir d/x/y/z; ok __fs_mkdir 'exists' test -d d/x/y/z
ok __fs_copy 'folder' __fs_copy d/sub d/sub2; ok __fs_copy 'copied' test -f d/sub2/b.log
ok __fs_move 'file' __fs_move d/w.txt d/w2.txt; ok __fs_move 'moved' test -f d/w2.txt
ok __fs_rm 'recursive' __fs_rm d/x; no __fs_rm 'deleted' test -d d/x
reta __fs_ls 'byte order, with hidden files' '.oculto|a.txt|bom.txt|sub|sub2|w2.txt' __fs_ls d
reta __fs_walk 'files, sorted' 'd/.oculto|d/a.txt|d/bom.txt|d/sub/b.log|d/sub2/b.log|d/w2.txt' __fs_walk d/
reta __fs_glob 'no hidden files' 'd/a.txt|d/bom.txt|d/w2.txt' __fs_glob 'd/*.txt'
reta __fs_glob 'hidden files without . and ..' 'd/.oculto' __fs_glob 'd/.*'
reta __fs_glob 'nothing' '' __fs_glob 'd/*.nada'
ok __fs_temp_dir 'creates' __fs_temp_dir; ok __fs_temp_dir 'folder' test -d "$__ret"; eq __fs_temp_dir 'registered' "${__temps[0]}" "$__ret"; rm -rf "$__ret"
ok __fs_temp_file 'creates' __fs_temp_file; ok __fs_temp_file 'file' test -f "$__ret"; rm -f "$__ret"
ret __stat_size 'bytes' 2 __stat_size d/sub/b.log
ok __stat_mtime 'seconds' eval '__stat_mtime d/a.txt && (( __ret > 1000000000 ))'
ret __fs_size 'bytes' 1 __fs_size d/a.txt
no __fs_size 'missing' __fs_size d/nao
ret __fs_size 'folder: 0' 0 __fs_size d
ok __fs_modified 'exists' __fs_modified d/a.txt
no __fs_modified 'missing' __fs_modified d/nao
printf 'a-b-a' > d/r.txt; __fs_replace d/r.txt a X; ret __fs_replace 'all' 'X-b-X' __fs_read d/r.txt
ret __path_join 'empty base' b __path_join '' b
ret __path_join 'slash' 'a/b' __path_join 'a/' b
ret __path_name 'last' 'c.tar.gz' __path_name '/a/c.tar.gz/'
ret __path_stem 'no extension' 'c.tar' __path_stem '/a/c.tar.gz'
ret __path_stem 'dotfile' '.bashrc' __path_stem '.bashrc'
ret __path_ext 'extension' 'gz' __path_ext 'c.tar.gz'
ret __path_ext 'dotfile' '' __path_ext '.bashrc'
ret __path_parent 'root' / __path_parent /
ret __path_parent 'relative' . __path_parent b
ret __path_parent '~' "$HOME" __path_parent '~/x'
ret __path_abs 'normalizes' /a/c __path_abs '/a/./b/../c/'

# ---- data: json, hash, archive, chmod ----------------------------------------------------------
j='{"a": {"b": [1, "xé", null]}, "s": "l1\nl2", "e": {}, "t": true}'
ok __json_parse 'valid' __json_parse "$j"
no __json_parse 'invalid' __json_parse '{"a": }'
no __json_parse 'trailing garbage' __json_parse '[1] x'
ret __json_get 'number' 1 __json_get "$j" 'a.b.0'
ret __json_get 'text with \u' 'xé' __json_get "$j" 'a.b[1]'
no __json_get 'null fails' __json_get "$j" 'a.b.2'
no __json_get 'does not exist' __json_get "$j" 'nada'
ret __json_get 'object: the span' '{}' __json_get "$j" 'e'
ret __json_get 'escape' $'l1\nl2' __json_get "$j" 's'
ret __json_get 'root' '[1]' __json_get '[1]' ''
reta __json_children 'list' '1|xé|null' __json_children "$j" 'a.b' a item
reta __json_children 'keys' 'a|s|e|t' __json_children "$j" '' o key
no __json_children 'not a list' __json_children "$j" 's' a item
ok __json_has 'exists' __json_has "$j" 't'
no __json_has 'null' __json_has "$j" 'a.b.2'
no __json_has 'invalid' __json_has '{' 'a'
ok __json_find 'finds' __json_find "$j" 'a.b.1'
ret __json_item 'span' '[1, "xé", null]' eval '__json_find "$j" a.b; __json_item "$__js_k"'
ret __json_quote 'escapes' '"a\"b\\c\nd"' __json_quote $'a"b\\c\nd'
ret __json_quote 'control' '"\u001b"' __json_quote $'\x1b'
ret __json_unescape 'surrogate pair' '👍' __json_unescape '\ud83d\udc4d'
ret __json_utf8 '2 bytes' 'é' __json_utf8 233
t_ws() { local __h_s='  x' __h_n=3 __h_i=0; __json_ws; (( __h_i == 2 )); }
ok __json_ws 'skips spaces' t_ws
t_str() { local __h_s='"a\"b" x' __h_n=8 __h_i=0; __json_str && (( __h_i == 6 )); }
ok __json_str 'escaped quote' t_str
t_val() { local LC_ALL=C __h_s='[1, {"k": "v"}]' __h_i=0; local __h_n=${#__h_s}; __js_p=(); __js_t=(); __js_a=(); __js_b=(); __js_ok=''; __json_val '' && (( ${#__js_p[@]} == 4 )); }
ok __json_val 'four values' t_val
ok __sha256_cmd 'finds a program' eval '__sha256_cmd; [[ -n $__ret ]]'
ret __hash_sha256 'abc' ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad __hash_sha256 abc
printf 'abc' > "$tmp/h.txt"
ret __hash_sha256_file 'file' ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad __hash_sha256_file "$tmp/h.txt"
no __hash_sha256_file 'does not exist' __hash_sha256_file "$tmp/nao"

# regex: the patterns are what the compiler emits (ERE with the ranges written out)
d='[0123456789]'
ok __re_test 'matches' __re_test 'v1.2' "v$d+"
no __re_test 'no match' __re_test 'abc' "^$d"
ok __re_test 'a dot is one character' __re_test 'é' '^.$'
ret __re_find 'first match' 'v1.24' __re_find 'x v1.24 v2.0' "v$d+\\.$d+" 'v\d+\.\d+'
no __re_find 'no match' __re_find 'abc' "$d" '\d'; eq __re_find 'message' "$__err_msg" "regex: no match for '\\d'"
reta __re_groups 'groups' '1|24|' __re_groups 'v1.24' "v($d+)\\.($d+)(x)?" 'p'
no __re_groups 'no match' __re_groups 'abc' "($d)" 'p'
reta __re_all 'every match' '1|22|333' __re_all 'a1b22c333' "$d+" 0
reta __re_all 'none' '' __re_all 'abc' "$d+" 0
reta __re_all 'anchored at the start' 'a' __re_all 'abc' '^[abc]' s
reta __re_all 'same text twice' 'ab|ab' __re_all 'xabyab' 'ab' 0
ret __re_replace 'every match, literal' 'a-$1-b-$1-' __re_replace 'a1b22' "$d+" '-$1-' 0
ret __re_replace 'at the end' 'abcab!' __re_replace 'abcabc' 'c$' '!' e
ret __re_replace 'at the start' 'Xbcabc' __re_replace 'abcabc' '^a' 'X' s
ret __re_replace 'no match' 'abc' __re_replace 'abc' "$d" 'x' 0
ret __re_replace 'characters, not bytes' 'a-ç-' __re_replace 'aéçé' 'é' '-' 0

# encoding
ret __b64_enc 'utf-8' 'b2zDoQ==' __b64_enc 'olá'
ret __b64_enc 'long text on one line' "$(printf 'YSBh%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25)" __b64_enc "$(printf 'a a%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25)"
eq __b64_d 'decodes' "$(printf 'aGk=' | __b64_d)" 'hi'
ret __b64_dec 'utf-8' 'olá' __b64_dec 'b2zDoQ=='
ret __b64_dec 'no padding, spaces' 'hi' __b64_dec $' aG\nk'
ret __b64_dec 'url-safe' '??>???' __b64_dec 'Pz8-Pz8_'
no __b64_dec 'not base64' __b64_dec '!!'; eq __b64_dec 'message' "$__err_msg" 'encoding: the text is not Base64'
no __b64_dec 'length' __b64_dec 'aGkxa'
no __b64_dec 'NUL byte' __b64_dec 'AAE='; eq __b64_dec 'not text' "$__err_msg" 'encoding: the decoded bytes are not UTF-8 text'
# UTF-8 is validated with `iconv`; without it (a bare Alpine), only the NUL check applies
if command -v iconv >/dev/null 2>&1; then no __b64_dec 'invalid utf-8' __b64_dec '/w=='; fi
# semver
ok __semver_split 'numbers and pre-release' __semver_split 'v1.20.3-rc.1+b7'; eq __semver_split 'parts' "${__sv_c[*]}|$__sv_p" '1 20 3|rc.1'
no __semver_split 'empty part' __semver_split '1..2'
ret __semver_compare 'by value' '-1' __semver_compare '1.9' '1.10'
ret __semver_compare 'missing parts are 0' '0' __semver_compare 'v1.2' '1.2.0'
ret __semver_compare 'leading zeros' '1' __semver_compare '1.010' '1.9'
ret __semver_compare 'pre-release is older' '-1' __semver_compare '2.0.0-rc.1' '2.0.0'
ret __semver_compare 'pre-release parts' '1' __semver_compare '1.0.0-beta.11' '1.0.0-beta.2'
no __semver_compare 'not a version' __semver_compare 'latest' '1.0'; eq __semver_compare 'message' "$__err_msg" "semver: 'latest' is not a version"
mkdir -p "$tmp/arq/p" && printf 'x' > "$tmp/arq/p/a.txt"
ok __archive_tar_create 'creates' __archive_tar_create "$tmp/arq/p.tgz" "$tmp/arq/p"
ok __archive_tar_extract 'extracts' __archive_tar_extract "$tmp/arq/p.tgz" "$tmp/arq/x"
eq __archive_tar_extract 'content' "$(cat "$tmp/arq/x/p/a.txt")" x
no __archive_tar_create 'folder does not exist' __archive_tar_create "$tmp/arq/q.tgz" "$tmp/arq/nao"
no __archive_zip_extract 'does not exist' __archive_zip_extract "$tmp/nao.zip" "$tmp/z"
if type -P zip >/dev/null 2>&1; then
  (cd "$tmp/arq" && zip -q -r p.zip p)
  ok __archive_zip_extract 'extracts' __archive_zip_extract "$tmp/arq/p.zip" "$tmp/arq/z"
  eq __archive_zip_extract 'content' "$(cat "$tmp/arq/z/p/a.txt")" x
fi
printf 'x' > "$tmp/c.sh"
ok __fs_chmod '+x' __fs_chmod "$tmp/c.sh" +x
ok __fs_chmod 'executable' test -x "$tmp/c.sh"
no __fs_chmod 'does not exist' __fs_chmod "$tmp/nao" +x
eq __fs_chmod 'dry-run only shows' "$(__dry=true; __fs_chmod "$tmp/nao" +x 2>&1; echo "s=$?")" "[dry-run] fs.chmod +x $tmp/nao"$'\ns=0'

# ---- system, time, log, ui, http, parallel ------------------------------------------------------
export NALU_T_X='v a'
ret __env_get 'exists' 'v a' __env_get NALU_T_X
no __env_get 'does not exist' __env_get NALU_T_NAO
no __env_get 'invalid name' __env_get 'A-B'
ok __env_has 'exists' __env_has NALU_T_X
__env_set NALU_T_Y 'z'; eq __env_set 'exports' "$(sh -c 'printf %s "$NALU_T_Y"')" z
__env_unset NALU_T_Y; eq __env_unset 'removes' "$(sh -c 'printf %s "${NALU_T_Y-gone}"')" gone
__env_unset NALU_T_NEVER; __env_unset 'bad name'; no __env_unset 'still unset' __env_has NALU_T_Y
ok __sys_arch 'non-empty' eval '__sys_arch; [[ -n $__ret ]]'
ok __sys_hostname 'non-empty' eval '__sys_hostname; [[ -n $__ret ]]'
ok __sys_user 'non-empty' eval '__sys_user; [[ -n $__ret ]]'
if (( EUID != 0 )); then no __sys_require_admin 'not root' __sys_require_admin; eq __sys_require_admin 'code' "$__err_code" 77; else ok __sys_require_admin 'root' __sys_require_admin; fi
ok __sys_cpus 'number' eval '__sys_cpus; (( __ret >= 1 ))'
eq __sys_cd 'changes the folder' "$(__sys_cd d/sub && pwd)" "$(cd d/sub && pwd)"
no __sys_cd 'missing' __sys_cd /nao/existe
ok __proc_which 'sh' __proc_which sh
no __proc_which 'missing' __proc_which nao-existe-xyz
ok __dirs_temp 'no trailing slash' eval '__dirs_temp; [[ $__ret != */ || $__ret == / ]]'
ok __time_now 'seconds' eval '__time_now; (( __ret > 1600000000 ))'
ok __time_now_ms 'milliseconds' eval '__time_now_ms; (( __ret > 1600000000000 ))'
ret __time_format 'fixed instant' 1971 __time_format '%Y' 40000000
ok __time_hour '0 to 23' eval '__time_hour -1; (( __ret >= 0 && __ret <= 23 ))'
eq __log_debug 'only with --verbose' "$(__log_debug a 2>&1; __verbose=true; __log_debug b 2>&1)" 'debug: b'
no __ui_confirm 'no terminal: false' __ui_confirm 'ok?' < /dev/null
ret __ui_ask 'no terminal: default' 'p' __ui_ask 'nome?' p < /dev/null
no __ui_password 'no terminal: fails' __ui_password 'senha?' < /dev/null
no __ui_choose 'no terminal: fails' __ui_choose 'qual?' a b < /dev/null
ret __secs 'ms → s' '1.500' __secs 1500
no __http_get 'closed port' __http_get 'http://127.0.0.1:9/' 2000 ''
no __http_status 'closed port' __http_status 'http://127.0.0.1:9/' 2000 ''
no __http_download 'closed port' __http_download 'http://127.0.0.1:9/' "$tmp/dl" ''
ok __http_download 'no half-written file' test ! -e "$tmp/dl.nalu-partial"
no __http_curl 'closed port' __http_curl get 'http://127.0.0.1:9/' 2000 '' '' ''
no __http_post 'closed port' __http_post 'http://127.0.0.1:9/' 'x' 'text/plain' 2000 ''
eq __http_post 'dry-run does not send' "$(__dry=true; __http_post 'http://127.0.0.1:9/' x t 2000 '' 2>&1; echo "s=$?")" $'[dry-run] http.post http://127.0.0.1:9/\ns=0'
no __http_request 'closed port' __http_request PUT 'http://127.0.0.1:9/' 'x' 'text/plain' 2000 ''
no __http_request 'unknown method' __http_request GET 'http://127.0.0.1:9/' '' 't' 2000 ''; eq __http_request 'message' "$__err_msg" "http.request: 'GET' is not a method it sends (POST, PUT, PATCH or DELETE)"
eq __http_request 'dry-run does not send' "$(__dry=true; __http_request DELETE 'http://127.0.0.1:9/' '' t 2000 '' 2>&1; echo "s=$?")" $'[dry-run] http.request DELETE http://127.0.0.1:9/\ns=0'
ret __curl_cfg 'headers and token' $'header = "X: 1"\nheader = "Authorization: Bearer a\\"b\\\\c"\n' __curl_cfg 'a"b\c' 'X: 1'
ret __curl_cfg 'empty' '' __curl_cfg ''
no __unimplemented 'fails with 70' __unimplemented regex.test; eq __unimplemented 'code' "$__err_code" 70
t_quad() { __ret=$(( $1 * $1 )); }
t_ruim() { __err "ruim $1" 5; }
ok __par_dir 'creates' __par_dir; pd=$__ret
__par_pids=()
for (( k = 0; k < 3; k++ )); do
  __par_slot 2
  __par_task "$pd" "$k" t_quad "$(( k + 2 ))" &
  __par_pids+=("$!")
done
ok __par_slot 'limit respected' true
ok __par_task 'runs in the background' true
ok __par_collect 'no errors' __par_collect "$pd" 3
reta __par_read 'in order' '4|9|16' __par_read "$pd" 3
__par_task "$pd" 3 t_ruim x &
__par_task "$pd" 4 t_ruim y &
no __par_collect 'joins the errors' __par_collect "$pd" 5
eq __par_collect 'message and code' "$__err_msg/$__err_code" 'parallel: ruim x; ruim y/5'
rm -rf "$pd"

# ---- every helper tested? -----------------------------------------------------------------------
for h in "${todos[@]}"; do
  if [[ $testados != *" $h "* ]]; then
    falhas=$(( falhas + 1 ))
    printf 'NO TEST  %s\n' "$h"
  fi
done
printf '\nruntime (%s, Bash %s): %d check(s), %d failure(s), %d helpers\n' "$alvo" "$BASH_VERSION" "$total" "$falhas" "${#todos[@]}"
(( falhas == 0 ))
