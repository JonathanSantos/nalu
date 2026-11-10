# Nalu: Bash runtime, files and paths. A leading `~` or `~/` is expanded here.
# Writes honor --dry-run, except inside folders/files created by fs.temp_dir/temp_file
# during this run (listed in __temps).

# @fn __tilde
# @target all
__tilde() {
  case $1 in
    '~') __p=$HOME ;;
    '~/'*) __p=$HOME/${1#'~/'} ;;
    *) __p=$1 ;;
  esac
}

# @fn __path_home
# @needs __tilde
# @target all
# A path passed to a program: `~` and `~/…` become the home folder.
__path_home() { __tilde "$1"; __ret=$__p; }

# @fn __dry_skip
# @target all
__dry_skip() {
  # __dry_skip description path… : status 0 = in --dry-run and outside the temporaries (show and skip)
  [[ $__dry == true ]] || return 1
  local __h_d=$1 __h_p __h_t __h_ok
  shift
  for __h_p in "$@"; do
    __h_ok=0
    for __h_t in "${__temps[@]}"; do
      if [[ $__h_p == "$__h_t" || $__h_p == "$__h_t"/* ]]; then __h_ok=1; break; fi
    done
    if (( ! __h_ok )); then printf '[dry-run] %s\n' "$__h_d" >&2; return 0; fi
  done
  return 1
}

# @fn __fs_exists
# @needs __tilde
# @target all
__fs_exists() {
  __tilde "$1"
  [[ -e $__p || -L $__p ]]
}

# @fn __fs_is_dir
# @needs __tilde
# @target all
__fs_is_dir() {
  __tilde "$1"
  [[ -d $__p ]]
}

# @fn __fs_is_file
# @needs __tilde
# @target all
__fs_is_file() {
  __tilde "$1"
  [[ -f $__p ]]
}

# @fn __fs_read
# @needs __tilde __err
# @target all
__fs_read() {
  __tilde "$1"
  if [[ -d $__p ]]; then __err "fs.read: '$1' is a folder"; return; fi
  if [[ ! -e $__p ]]; then __err "fs.read: '$1' does not exist"; return; fi
  if [[ ! -r $__p ]]; then __err "fs.read: could not read '$1' (permission denied)"; return; fi
  __ret=''
  IFS= read -r -d '' __ret < "$__p"
  __ret=${__ret#$'\xef\xbb\xbf'}
  return 0
}

# @fn __fs_write
# @needs __tilde __err __dry_skip
# @target all
__fs_write() {
  __tilde "$1"
  __dry_skip "fs.write $1" "$__p" && return 0
  { printf '%s' "$2" > "$__p"; } 2>/dev/null || __err "fs.write: could not write '$1'"
}

# @fn __fs_append
# @needs __tilde __err __dry_skip
# @target all
__fs_append() {
  __tilde "$1"
  __dry_skip "fs.append $1" "$__p" && return 0
  { printf '%s' "$2" >> "$__p"; } 2>/dev/null || __err "fs.append: could not write '$1'"
}

# @fn __fs_rm
# @needs __tilde __err __dry_skip
# @target all
__fs_rm() {
  __tilde "$1"
  local __h_q=$__p
  while [[ $__h_q == */ && $__h_q != / ]]; do __h_q=${__h_q%/}; done
  while [[ $__h_q == ./* ]]; do __h_q=${__h_q#./}; done
  case $__h_q in
    ''|/|.|..|'~') __err "fs.rm refused to delete '$1'"; return ;;
  esac
  if [[ $__h_q == "${HOME%/}" || $__h_q == "${PWD%/}" ]]; then __err "fs.rm refused to delete '$1'"; return; fi
  [[ -e $__p || -L $__p ]] || return 0
  __dry_skip "fs.rm $1" "$__p" && return 0
  rm -rf -- "$__p" 2>/dev/null || __err "fs.rm: could not delete '$1'"
}

# @fn __fs_mkdir
# @needs __tilde __err __dry_skip
# @target all
__fs_mkdir() {
  __tilde "$1"
  [[ -d $__p ]] && return 0
  __dry_skip "fs.mkdir $1" "$__p" && return 0
  mkdir -p -- "$__p" 2>/dev/null || __err "fs.mkdir: could not create '$1'"
}

# @fn __fs_copy
# @needs __tilde __err __dry_skip
# @target all
__fs_copy() {
  local __h_a __h_b
  __tilde "$1"; __h_a=$__p
  __tilde "$2"; __h_b=$__p
  # in --dry-run only shows (as in PowerShell): the source may come from a step that was also only shown
  __dry_skip "fs.copy $1 $2" "$__h_b" && return 0
  if [[ ! -e $__h_a ]]; then __err "fs.copy: '$1' does not exist"; return; fi
  cp -R -- "$__h_a" "$__h_b" 2>/dev/null || __err "fs.copy: could not copy '$1' to '$2'"
}

# @fn __fs_move
# @needs __tilde __err __dry_skip
# @target all
__fs_move() {
  local __h_a __h_b
  __tilde "$1"; __h_a=$__p
  __tilde "$2"; __h_b=$__p
  __dry_skip "fs.move $1 $2" "$__h_a" "$__h_b" && return 0
  if [[ ! -e $__h_a && ! -L $__h_a ]]; then __err "fs.move: '$1' does not exist"; return; fi
  mv -f -- "$__h_a" "$__h_b" 2>/dev/null || __err "fs.move: could not move '$1' to '$2'"
}

# @fn __fs_ls
# @needs __tilde __err
# @target all
__fs_ls() {
  __tilde "$1"
  if [[ ! -d $__p ]]; then __err "fs.ls: '$1' is not a folder"; return; fi
  local LC_ALL=C __h_f __h_dg=1
  shopt -q dotglob && __h_dg=0
  shopt -s dotglob
  __ret_a=()
  for __h_f in "${__p%/}"/*; do
    if [[ -e $__h_f || -L $__h_f ]]; then __ret_a+=("${__h_f##*/}"); fi
  done
  (( __h_dg )) && shopt -u dotglob
  return 0
}

# @fn __fs_walk
# @needs __tilde __err
# @target all
__fs_walk() {
  __tilde "$1"
  if [[ ! -d $__p ]]; then __err "fs.walk: '$1' is not a folder"; return; fi
  local __h_d=$__p __h_f
  while [[ $__h_d == */ && $__h_d != / ]]; do __h_d=${__h_d%/}; done
  [[ $__h_d == -* ]] && __h_d=./$__h_d
  __ret_a=()
  while IFS= read -r -d '' __h_f; do
    if [[ -f $__h_f ]]; then __ret_a+=("$__h_f"); fi
  done < <(find "$__h_d" \( -type f -o -type l \) -print0 2>/dev/null | LC_ALL=C sort -z)
  return 0
}

# @fn __fs_glob
# @needs __tilde
# @target all
__fs_glob() {
  __tilde "$1"
  local __h_dir='' __h_base=$__p __h_f __h_ng=1
  if [[ $__p == */* ]]; then __h_dir=${__p%/*}/; __h_base=${__p##*/}; fi
  __h_base=${__h_base//\\/\\\\}
  __h_base=${__h_base//\[/\\[}
  local LC_ALL=C IFS=''
  shopt -q nullglob && __h_ng=0
  shopt -s nullglob
  __ret_a=()
  for __h_f in "$__h_dir"$__h_base; do
    case ${__h_f##*/} in
      .|..) ;;
      *) __ret_a+=("$__h_f") ;;
    esac
  done
  (( __h_ng )) && shopt -u nullglob
  return 0
}

# @fn __fs_temp_dir
# @needs __err
# @target all
__fs_temp_dir() {
  local __h_t=${TMPDIR:-/tmp}
  __h_t=${__h_t%/}
  __ret=$(mktemp -d "$__h_t/nalu.XXXXXX" 2>/dev/null) || { __err "fs.temp_dir: could not create a temporary folder"; return; }
  __temps+=("$__ret")
}

# @fn __fs_temp_file
# @needs __err
# @target all
__fs_temp_file() {
  local __h_t=${TMPDIR:-/tmp}
  __h_t=${__h_t%/}
  __ret=$(mktemp "$__h_t/nalu.XXXXXX" 2>/dev/null) || { __err "fs.temp_file: could not create a temporary file"; return; }
  __temps+=("$__ret")
}

# @fn __stat_size
# @target linux
__stat_size() {
  __ret=$(stat -c %s -- "$1" 2>/dev/null) || __ret=$(wc -c < "$1" 2>/dev/null) || return 1
  __ret=$(( __ret ))
}

# @fn __stat_size
# @target macos
__stat_size() {
  __ret=$(stat -f %z -- "$1" 2>/dev/null) || return 1
}

# @fn __stat_mtime
# @target linux
__stat_mtime() {
  # GNU and BusyBox: `stat -c`; without it (a Bash 4+ outside GNU), `date -r`, which both also have.
  __ret=$(stat -c %Y -- "$1" 2>/dev/null) || __ret=$(date -r "$1" +%s 2>/dev/null) || return 1
}

# @fn __stat_mtime
# @target macos
__stat_mtime() {
  __ret=$(stat -f %m -- "$1" 2>/dev/null) || return 1
}

# @fn __fs_size
# @needs __tilde __err __stat_size
# @target all
__fs_size() {
  __tilde "$1"
  if [[ ! -e $__p ]]; then __err "fs.size: '$1' does not exist"; return; fi
  if [[ -d $__p ]]; then __ret=0; return 0; fi
  __stat_size "$__p" || __err "fs.size: could not read the size of '$1'"
}

# @fn __fs_modified
# @needs __tilde __err __stat_mtime
# @target all
__fs_modified() {
  __tilde "$1"
  if [[ ! -e $__p && ! -L $__p ]]; then __err "fs.modified: '$1' does not exist"; return; fi
  __stat_mtime "$__p" || __err "fs.modified: could not read the modification time of '$1'"
}

# @fn __fs_chmod
# @needs __err __tilde __dry_skip
# @target all
__fs_chmod() {
  __tilde "$1"
  # in --dry-run only shows: the file may come from a write that was also only shown
  __dry_skip "fs.chmod $2 $1" "$__p" && return 0
  [[ -e $__p ]] || { __err "fs.chmod: '$1' does not exist"; return; }
  chmod -- "$2" "$__p" 2>/dev/null || __err "fs.chmod: could not apply '$2' to '$1'"
}

# @fn __fs_replace
# @needs __tilde __err __dry_skip
# @target all
__fs_replace() {
  __tilde "$1"
  __dry_skip "fs.replace $1" "$__p" && return 0
  if [[ ! -f $__p ]]; then __err "fs.replace: '$1' does not exist"; return; fi
  local __h_s='' __h_o='' __h_f=$2 __h_t=$3
  IFS= read -r -d '' __h_s < "$__p"
  [[ -n $__h_f && $__h_s == *"$__h_f"* ]] || return 0
  while [[ $__h_s == *"$__h_f"* ]]; do
    __h_o+=${__h_s%%"$__h_f"*}$__h_t
    __h_s=${__h_s#*"$__h_f"}
  done
  { printf '%s' "$__h_o$__h_s" > "$__p"; } 2>/dev/null || __err "fs.replace: could not write '$1'"
}

# @fn __path_join
# @target all
__path_join() {
  if [[ -z $1 ]]; then __ret=$2; else __ret=${1%/}/$2; fi
}

# @fn __path_name
# @needs __tilde
# @target all
__path_name() {
  __tilde "$1"
  local __h_q=$__p
  while [[ $__h_q == */ && $__h_q != / ]]; do __h_q=${__h_q%/}; done
  if [[ $__h_q == / ]]; then __ret=''; return 0; fi
  __ret=${__h_q##*/}
}

# @fn __path_stem
# @needs __path_name
# @target all
__path_stem() {
  __path_name "$1"
  case $__ret in
    .|..) ;;
    ?*.*) __ret=${__ret%.*} ;;
  esac
}

# @fn __path_ext
# @needs __path_name
# @target all
__path_ext() {
  __path_name "$1"
  case $__ret in
    .|..) __ret='' ;;
    ?*.*) __ret=${__ret##*.} ;;
    *) __ret='' ;;
  esac
}

# @fn __path_parent
# @needs __tilde
# @target all
__path_parent() {
  __tilde "$1"
  local __h_q=$__p
  while [[ $__h_q == */ && $__h_q != / ]]; do __h_q=${__h_q%/}; done
  if [[ $__h_q == / ]]; then __ret=/; return 0; fi
  if [[ $__h_q != */* ]]; then __ret=.; return 0; fi
  __h_q=${__h_q%/*}
  while [[ $__h_q == */ && $__h_q != / ]]; do __h_q=${__h_q%/}; done
  __ret=${__h_q:-/}
}

# @fn __path_abs
# @needs __tilde
# @target all
__path_abs() {
  __tilde "$1"
  local __h_q=$__p __h_s __h_o=''
  [[ $__h_q == /* ]] || __h_q=$PWD/$__h_q
  local -a __h_st=()
  while [[ -n $__h_q ]]; do
    __h_s=${__h_q%%/*}
    if [[ $__h_q == */* ]]; then __h_q=${__h_q#*/}; else __h_q=''; fi
    case $__h_s in
      ''|.) ;;
      ..) (( ${#__h_st[@]} > 0 )) && unset "__h_st[${#__h_st[@]}-1]" ;;
      *) __h_st+=("$__h_s") ;;
    esac
  done
  for __h_s in "${__h_st[@]}"; do __h_o+=/$__h_s; done
  __ret=${__h_o:-/}
}

