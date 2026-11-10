# Nalu: Bash runtime, env, sys, proc, dirs, time, log, ui, http, parallel.

# @fn __env_get
# @needs __err
# @target all
__env_get() {
  [[ $1 != *[!A-Za-z0-9_]* && $1 == [A-Za-z_]* && -n ${!1+x} ]] && { __ret=${!1}; return 0; }
  __err "environment variable $1 is not set"
}

# @fn __env_has
# @target all
__env_has() { [[ $1 != *[!A-Za-z0-9_]* && $1 == [A-Za-z_]* && -n ${!1+x} ]]; }

# @fn __env_set
# @target all
__env_set() { [[ $1 != *[!A-Za-z0-9_]* && $1 == [A-Za-z_]* ]] && export "$1=$2"; return 0; }

# @fn __env_unset
# @target all
__env_unset() { [[ $1 != *[!A-Za-z0-9_]* && $1 == [A-Za-z_]* ]] && unset "$1"; return 0; }

# @fn __sys_arch
# @target all
__sys_arch() {
  __ret=$(uname -m 2>/dev/null)
  case $__ret in
    x86_64|amd64) __ret=x64 ;;
    aarch64|arm64) __ret=arm64 ;;
    i?86) __ret=x86 ;;
    armv7*) __ret=arm ;;
  esac
}

# @fn __sys_hostname
# @target all
__sys_hostname() {
  __ret=${HOSTNAME:-}
  [[ -n $__ret ]] || __ret=$(uname -n 2>/dev/null)
}

# @fn __sys_user
# @target all
__sys_user() {
  __ret=${USER:-}
  [[ -n $__ret ]] || __ret=$(id -un 2>/dev/null)
}

# @fn __sys_require_admin
# @needs __err
# @target all
__sys_require_admin() {
  (( EUID == 0 )) || __err "this script must be run as administrator (root)" 77
}

# @fn __sys_cpus
# @target linux
__sys_cpus() {
  __ret=$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)
  [[ $__ret =~ ^[0-9]+$ ]] || __ret=1
}

# @fn __sys_cpus
# @target macos
__sys_cpus() {
  __ret=$(sysctl -n hw.ncpu 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)
  [[ $__ret =~ ^[0-9]+$ ]] || __ret=1
}

# @fn __sys_cd
# @needs __tilde __err
# @target all
__sys_cd() {
  __tilde "$1"
  CDPATH='' cd -- "$__p" 2>/dev/null || __err "sys.cd: could not enter '$1'"
}

# @fn __proc_which
# @needs __err
# @target all
__proc_which() {
  __ret=$(type -P -- "$1" 2>/dev/null)
  [[ -n $__ret ]] || __err "'$1' was not found in PATH" 127
}

# @fn __dirs_temp
# @target all
__dirs_temp() {
  __ret=${TMPDIR:-/tmp}
  [[ $__ret == / ]] || __ret=${__ret%/}
}

# @fn __time_now
# @target linux
__time_now() {
  printf -v __ret '%(%s)T' -1
}

# @fn __time_now
# @target macos
__time_now() {
  __ret=$(date +%s)
}

# @fn __time_now_ms
# @target linux
__time_now_ms() {
  if [[ -n ${EPOCHREALTIME:-} ]]; then
    __ret=${EPOCHREALTIME/[.,]/}
    __ret=$(( 10#$__ret / 1000 ))
    return 0
  fi
  __ret=$(date +%s%N 2>/dev/null)
  if [[ $__ret =~ ^[0-9]{19}$ ]]; then
    __ret=$(( 10#$__ret / 1000000 ))
  else
    printf -v __ret '%(%s)T' -1
    __ret=$(( __ret * 1000 ))
  fi
}

# @fn __time_now_ms
# @target macos
__time_now_ms() {
  # macOS date has no %N; the perl that ships with macOS gives milliseconds (without it: whole seconds).
  if __ret=$(perl -MTime::HiRes=time -e 'printf("%d", time() * 1000)' 2>/dev/null) && [[ $__ret =~ ^[0-9]+$ ]]; then
    return 0
  fi
  __ret=$(( $(date +%s) * 1000 ))
}

# @fn __time_format
# @target linux
__time_format() {
  # format instant (-1 = now); `printf %(…)T` is built into Bash 4.2+
  if [[ $1 != *')'* ]]; then printf -v __ret "%($1)T" "$2"
  elif (( $2 < 0 )); then __ret=$(date +"$1"); else __ret=$(date -d "@$2" +"$1"); fi
}

# @fn __time_format
# @target macos
__time_format() { if (( $2 < 0 )); then __ret=$(date +"$1"); else __ret=$(date -r "$2" +"$1"); fi; }

# @fn __time_hour
# @needs __time_format
# @target all
__time_hour() { __time_format '%H' "$1"; __ret=$(( 10#$__ret )); }

# @fn __log_debug
# @target all
__log_debug() { if [[ $__verbose == true ]]; then printf 'debug: %s\n' "$1" >&2; fi; return 0; }

# @fn __ui_confirm
# @target all
__ui_confirm() {
  local __h_r
  [[ -t 0 ]] || return 1
  printf '%s [y/N] ' "$1" >&2
  IFS= read -r __h_r || return 1
  case $__h_r in
    s|S|sim|Sim|SIM|y|Y|yes|Yes|YES) return 0 ;;
  esac
  return 1
}

# @fn __ui_ask
# @target all
__ui_ask() {
  local __h_r
  __ret=$2
  [[ -t 0 ]] || return 0
  printf '%s ' "$1" >&2
  IFS= read -r __h_r || return 0
  [[ -n $__h_r ]] && __ret=$__h_r
  return 0
}

# @fn __ui_password
# @needs __err
# @target all
__ui_password() {
  [[ -t 0 ]] || { __err "ui.password: no terminal to ask: $1"; return; }
  printf '%s ' "$1" >&2
  IFS= read -r -s __ret || { printf '\n' >&2; __err "ui.password: input cancelled"; return; }
  printf '\n' >&2
  return 0
}

# @fn __ui_choose
# @needs __err
# @target all
__ui_choose() {
  # __ui_choose question option…
  local __h_q=$1 __h_i __h_r
  shift
  (( $# > 0 )) || { __err "ui.choose: no options"; return; }
  [[ -t 0 ]] || { __err "ui.choose: no terminal to ask: $__h_q"; return; }
  while :; do
    printf '%s\n' "$__h_q" >&2
    for (( __h_i = 1; __h_i <= $#; __h_i++ )); do printf '  %d) %s\n' "$__h_i" "${!__h_i}" >&2; done
    printf '> ' >&2
    IFS= read -r __h_r || { __err "ui.choose: input cancelled"; return; }
    if [[ $__h_r =~ ^[0-9]+$ ]] && (( 10#$__h_r >= 1 && 10#$__h_r <= $# )); then
      __h_r=$(( 10#$__h_r ))
      __ret=${!__h_r}
      return 0
    fi
  done
}

# @fn __secs
# @target all
__secs() {
  # ms → fractional seconds (for curl/wget/sleep)
  printf -v __ret '%d.%03d' $(( $1 / 1000 )) $(( $1 % 1000 ))
}

# @fn __curl_cfg
# @target all
__curl_cfg() {
  # __curl_cfg bearer headers… → __ret: curl config (read via -K) with `header = "…"`.
  # This way the token and headers never show up on the command line (ps, /proc) or in --verbose.
  local __h_b=$1 __h_x __h_bs='\' __h_q='"' __h_c=''
  shift
  if [[ -n $__h_b ]]; then set -- "$@" "Authorization: Bearer $__h_b"; fi
  for __h_x in "$@"; do
    __h_x=${__h_x//"$__h_bs"/"$__h_bs$__h_bs"}
    __h_x=${__h_x//"$__h_q"/"$__h_bs$__h_q"}
    __h_c+="header = \"$__h_x\""$'\n'
  done
  __ret=$__h_c
}

# @fn __http_curl
# @needs __err __secs __curl_cfg
# @target all
__http_curl() {
  # __http_curl mode url ms bearer body content-type headers… → __ret (body, or the code in status mode)
  # mode: get, status, post, download (in download, `body` is the destination file)
  local __h_m=$1 __h_u=$2 __h_b=$4 __h_body=$5 __h_ct=$6 __h_t __h_o __h_cfg __h_st __h_tmp
  __secs "$3"
  __h_t=$__ret
  shift 6
  # request: the method comes in __http_x (set by __http_request); an empty body sends no body
  if [[ $__h_m == post || ( $__h_m == request && -n $__h_body ) ]]; then set -- "$@" "Content-Type: $__h_ct"; fi
  if ! type -P curl >/dev/null 2>&1; then
    # no curl: wget covers GET without headers or token
    if [[ $__h_m == post || $__h_m == request || -n $__h_b || $# -gt 0 ]]; then __err "http.$__h_m: needs curl (POST, headers and tokens)"; return; fi
    case $__h_m in
      get)
        __h_o=$(wget -q -O - -T "${__h_t%.*}" -- "$__h_u" 2>/dev/null && printf '.') || { __err "http.get: could not reach $__h_u"; return; }
        __ret=${__h_o%.}
        return 0 ;;
      status)
        __h_o=$(wget -S --spider -T "${__h_t%.*}" -- "$__h_u" 2>&1)
        __ret=$(printf '%s\n' "$__h_o" | sed -n 's/^ *HTTP\/[0-9.]* \([0-9][0-9][0-9]\).*/\1/p' | tail -n 1)
        [[ -n $__ret ]] || { __err "http.status: could not reach $__h_u"; return; }
        __ret=$(( 10#$__ret ))
        return 0 ;;
      download)
        wget -q -O "$__h_body" -T "${__h_t%.*}" -- "$__h_u" 2>/dev/null || { rm -f -- "$__h_body"; __err "http.download: could not download $__h_u"; return; }
        return 0 ;;
    esac
  fi
  __curl_cfg "$__h_b" "$@"
  __h_cfg=$__ret
  case $__h_m in
    get)
      __h_o=$(curl -sSL --max-time "$__h_t" -w '%{http_code}' -K <(printf '%s' "$__h_cfg") -- "$__h_u" 2>/dev/null) || { __err "http.get: could not reach $__h_u"; return; } ;;
    post)
      __h_o=$(printf '%s' "$__h_body" | curl -sSL --max-time "$__h_t" -w '%{http_code}' -K <(printf '%s' "$__h_cfg") --data-binary @- -- "$__h_u" 2>/dev/null) || { __err "http.post: could not reach $__h_u"; return; } ;;
    request)
      if [[ -n $__h_body ]]; then
        __h_o=$(printf '%s' "$__h_body" | curl -sSL -X "$__http_x" --max-time "$__h_t" -w '%{http_code}' -K <(printf '%s' "$__h_cfg") --data-binary @- -- "$__h_u" 2>/dev/null) || { __err "http.request: could not reach $__h_u"; return; }
      else
        __h_o=$(curl -sSL -X "$__http_x" --max-time "$__h_t" -w '%{http_code}' -K <(printf '%s' "$__h_cfg") -- "$__h_u" 2>/dev/null) || { __err "http.request: could not reach $__h_u"; return; }
      fi ;;
    status)
      __h_o=$(curl -sSL -o /dev/null --max-time "$__h_t" -w '%{http_code}' -K <(printf '%s' "$__h_cfg") -- "$__h_u" 2>/dev/null)
      if [[ ! $__h_o =~ ^[0-9]+$ ]] || (( 10#$__h_o == 0 )); then __err "http.status: could not reach $__h_u"; return; fi
      __ret=$(( 10#$__h_o ))
      return 0 ;;
    download)
      # download alongside, swap in only at the end: a failed download leaves no half-written file
      __h_tmp="$__h_body.nalu-partial"
      __h_st=$(curl -sSL -o "$__h_tmp" --max-time "$__h_t" -w '%{http_code}' -K <(printf '%s' "$__h_cfg") -- "$__h_u" 2>/dev/null) || { rm -f -- "$__h_tmp"; __err "http.download: could not download $__h_u"; return; }
      if (( 10#$__h_st >= 400 )); then rm -f -- "$__h_tmp"; __err "http.download: $__h_u answered $__h_st"; return; fi
      mv -f -- "$__h_tmp" "$__h_body" || { rm -f -- "$__h_tmp"; __err "http.download: could not write $__h_body"; return; }
      return 0 ;;
  esac
  __h_st=${__h_o:${#__h_o}-3}
  __ret=${__h_o:0:${#__h_o}-3}
  if (( 10#$__h_st >= 400 )); then __err "http.$__h_m: $__h_u answered $__h_st"; return; fi
  return 0
}

# @fn __http_get
# @needs __http_curl
# @target all
__http_get() {
  # __http_get url ms bearer headers…
  local __h_u=$1 __h_t=$2 __h_b=$3
  shift 3
  __http_curl get "$__h_u" "$__h_t" "$__h_b" '' '' "$@"
}

# @fn __http_status
# @needs __http_curl
# @target all
__http_status() {
  local __h_u=$1 __h_t=$2 __h_b=$3
  shift 3
  __http_curl status "$__h_u" "$__h_t" "$__h_b" '' '' "$@"
}

# @fn __http_post
# @needs __http_curl
# @target all
__http_post() {
  # __http_post url body content-type ms bearer headers…; in --dry-run only shows (nothing leaves the machine)
  local __h_u=$1 __h_body=$2 __h_ct=$3 __h_t=$4 __h_b=$5
  shift 5
  if [[ $__dry == true ]]; then printf '[dry-run] http.post %s\n' "$__h_u" >&2; __ret=''; return 0; fi
  __http_curl post "$__h_u" "$__h_t" "$__h_b" "$__h_body" "$__h_ct" "$@"
}

# @fn __http_request
# @needs __http_curl __err
# @target all
__http_request() {
  # __http_request method url body content-type ms bearer headers…; in --dry-run only shows (nothing leaves the machine)
  local __http_x=$1 __h_u=$2 __h_body=$3 __h_ct=$4 __h_t=$5 __h_b=$6
  shift 6
  case $__http_x in
    POST | PUT | PATCH | DELETE) ;;
    *) __err "http.request: '$__http_x' is not a method it sends (POST, PUT, PATCH or DELETE)"; return ;;
  esac
  if [[ $__dry == true ]]; then printf '[dry-run] http.request %s %s\n' "$__http_x" "$__h_u" >&2; __ret=''; return 0; fi
  __http_curl request "$__h_u" "$__h_t" "$__h_b" "$__h_body" "$__h_ct" "$@"
}

# @fn __http_download
# @needs __http_curl __tilde __dry_skip
# @target all
__http_download() {
  # __http_download url dest bearer headers…
  local __h_u=$1 __h_b=$3
  __tilde "$2"
  local __h_to=$__p
  shift 3
  __dry_skip "http.download $__h_u $__h_to" "$__h_to" && return 0
  __http_curl download "$__h_u" 300000 "$__h_b" "$__h_to" '' "$@"
}

# @fn __unimplemented
# @needs __err
# @target all
__unimplemented() {
  __err "$1 is not implemented yet" 70
}

# @fn __par_slot
# @target linux
__par_slot() {
  # waits for a free slot (at most $1 tasks at once); the caller adds the new task's pid to __par_pids.
  # `wait -n` can return at once in Bash 5.1+ (a status kept from an earlier `wait`), so the live tasks are
  # counted again after it, and when nothing ended the oldest task is waited for instead.
  local __h_m=$1 __h_p __h_live __h_n=0
  (( __h_m < 1 )) && __h_m=1
  while (( ${#__par_pids[@]} >= __h_m )); do
    __h_live=()
    for __h_p in "${__par_pids[@]}"; do kill -0 "$__h_p" 2>/dev/null && __h_live+=("$__h_p"); done
    __par_pids=("${__h_live[@]}")
    (( ${#__par_pids[@]} < __h_m )) && break
    if (( __h_n == 0 )); then
      wait -n 2>/dev/null
      __h_n=1
    else
      wait "${__par_pids[0]}" 2>/dev/null
      __par_pids=("${__par_pids[@]:1}")
      __h_n=0
    fi
  done
}

# @fn __par_slot
# @target macos
__par_slot() {
  # Bash 3.2 has no `wait -n`: drops the tasks that ended, then waits for the oldest while every slot is busy.
  local __h_m=$1 __h_p __h_live
  (( __h_m < 1 )) && __h_m=1
  while (( ${#__par_pids[@]} >= __h_m )); do
    __h_live=()
    for __h_p in "${__par_pids[@]}"; do kill -0 "$__h_p" 2>/dev/null && __h_live+=("$__h_p"); done
    __par_pids=("${__h_live[@]}")
    (( ${#__par_pids[@]} < __h_m )) && break
    wait "${__par_pids[0]}" 2>/dev/null
    __par_pids=("${__par_pids[@]:1}")
  done
}

# @fn __par_task
# @target all
__par_task() {
  # __par_task dir i function args… (runs in the background): writes the result or the error
  local __h_d=$1 __h_i=$2
  shift 2
  if "$@"; then
    printf '%s' "${__ret-}" > "$__h_d/$__h_i.out"
    : > "$__h_d/$__h_i.ok"
  else
    printf '%s' "$__err_msg" > "$__h_d/$__h_i.err"
    printf '%s' "$__err_code" > "$__h_d/$__h_i.code"
  fi
}

# @fn __par_collect
# @needs __err
# @target all
__par_collect() {
  # __par_collect dir n : gathers the errors of tasks 0..n-1 (in order); status 1 if there was any.
  local __h_d=$1 __h_n=$2 __h_i __h_m __h_msg='' __h_code=''
  wait
  for (( __h_i = 0; __h_i < __h_n; __h_i++ )); do
    if [[ -f $__h_d/$__h_i.err ]]; then
      IFS= read -r -d '' __h_m < "$__h_d/$__h_i.err"
      __h_msg+="${__h_msg:+; }$__h_m"
      if [[ -z $__h_code && -f $__h_d/$__h_i.code ]]; then IFS= read -r __h_code < "$__h_d/$__h_i.code"; fi
    elif [[ ! -f $__h_d/$__h_i.ok ]]; then
      __h_msg+="${__h_msg:+; }task $(( __h_i + 1 )) finished without a result"
    fi
  done
  if [[ -n $__h_msg ]]; then
    __err "parallel: $__h_msg" "${__h_code:-1}"
    return 1
  fi
  return 0
}

# @fn __par_read
# @target all
__par_read() {
  # __par_read dir n : the tasks' results in order (__ret_a)
  local __h_i __h_v
  __ret_a=()
  for (( __h_i = 0; __h_i < $2; __h_i++ )); do
    __h_v=''
    [[ -f $1/$__h_i.out ]] && IFS= read -r -d '' __h_v < "$1/$__h_i.out"
    __ret_a+=("$__h_v")
  done
}

# @fn __par_dir
# @needs __err
# @target all
__par_dir() {
  local __h_t=${TMPDIR:-/tmp}
  __h_t=${__h_t%/}
  __ret=$(mktemp -d "$__h_t/nalu-par.XXXXXX" 2>/dev/null) || { __err "parallel: could not create a temporary folder"; return; }
}
