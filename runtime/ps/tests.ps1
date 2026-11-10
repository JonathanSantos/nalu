# Unit tests for the PowerShell runtime. Runs with PowerShell (5.1 or 7):
#   pwsh -NoProfile -File runtime/ps/tests.ps1
# This file is UTF-8 with a BOM: without it, Windows PowerShell 5.1 reads the literals below as ANSI.
# `cargo test` runs this file when PowerShell is available (NALU_PWSH, pwsh or powershell).
Set-StrictMode -Version 2
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
foreach ($f in 'core', 'text', 'list', 'path', 'fs', 'sys', 'cmd', 'params', 'data') { . (Join-Path $here "$f.ps1") }
$script:__dry = $false
$script:__verbose = $false
$script:falhas = 0
$script:total = 0

function Check([string]$name, $got, $want) {
    $script:total++
    if (-not (__Eq $got $want)) {
        [Console]::Error.WriteLine("FAILED ${name}: got [$(__Txt $got)] expected [$(__Txt $want)]")
        $script:falhas++
    }
}

function L { $l = [System.Collections.Generic.List[object]]::new(); foreach ($x in $args) { $l.Add($x) }; return ,$l }

# data: json, hash, archive, chmod
function Fails([scriptblock]$b) { try { & $b; return $false } catch { return $true } }
$j = '{"a": {"b": [1, "xé", null]}, "s": "l1\nl2", "e": {}, "t": true}'
Check 'json number' (__JsonGet $j 'a.b.0') '1'
Check 'json \u' (__JsonGet $j 'a.b[1]') 'xé'
Check 'json null falha' (Fails { __JsonGet $j 'a.b.2' }) $true
Check 'json does not exist' (Fails { __JsonGet $j 'nada' }) $true
Check 'json invalid' (Fails { __JsonGet '{"a": }' 'a' }) $true
Check 'json trailing garbage' (Fails { __JsonGet '[1] x' '' }) $true
Check 'json object' (__JsonGet $j 'e') '{}'
Check 'json escape' (__JsonGet $j 's') "l1`nl2"
Check 'json root' (__JsonGet '[1]' '') '[1]'
Check 'json list' ((__JsonChildren $j 'a.b' 'a' 'item') -join '|') '1|xé|null'
Check 'json keys' ((__JsonChildren $j '' 'o' 'key') -join '|') 'a|s|e|t'
Check 'json not a list' (Fails { __JsonChildren $j 's' 'a' 'item' }) $true
Check 'json has' (__JsonHas $j 't') $true
Check 'json has null' (__JsonHas $j 'a.b.2') $false
Check 'json has invalid' (__JsonHas '{' 'a') $false

# regex: the patterns are what the compiler emits (.NET syntax, ranges and anchors written out)
Check 're find' (__ReFind 'x v1.24 v2.0' 'v[0-9]+\.[0-9]+' 'p') 'v1.24'
Check 're find no match' (Fails { __ReFind 'abc' '[0-9]' 'p' }) $true
Check 're groups' ((__ReGroups 'v1.24' 'v([0-9]+)\.([0-9]+)(x)?' 'p') -join '|') '1|24|'
Check 're groups count' (__ReGroups 'v1.24' 'v([0-9]+)\.([0-9]+)(x)?' 'p').Count 3
Check 're all' ((__ReAll 'a1b22c333' '[0-9]+') -join '|') '1|22|333'
Check 're all none' (__ReAll 'abc' '[0-9]+').Count 0
Check 're all anchored' ((__ReAll 'abc' '\A[abc]') -join '|') 'a'
Check 're replace literal' (__ReReplace 'a1b22' '[0-9]+' '-$1-') 'a-$1-b-$1-'
Check 're replace end' (__ReReplace 'abcabc' 'c\z' '!') 'abcab!'

# encoding
Check 'b64 enc' (__B64Enc ('ol' + [char]0xE1)) 'b2zDoQ=='
Check 'b64 dec' (__B64Dec 'b2zDoQ==') ('ol' + [char]0xE1)
Check 'b64 dec spaces' (__B64Dec " aG`nk") 'hi'
Check 'b64 dec url-safe' (__B64Dec 'Pz8-Pz8_') '??>???'
Check 'b64 dec not base64' (Fails { __B64Dec '!!' }) $true
Check 'b64 dec length' (Fails { __B64Dec 'aGkxa' }) $true
Check 'b64 dec NUL' (Fails { __B64Dec 'AAE=' }) $true
Check 'b64 dec invalid utf-8' (Fails { __B64Dec '/w==' }) $true

# nested collections (D34)
$rs = [char]0x1e
Check 'encl items' (__EncL (L 'a b' '' 'c')) ('a b' + $rs + $rs + 'c' + $rs)
Check 'encl ints and bools' (__EncL (L ([long]12) $true $false)) ('12' + $rs + 'true' + $rs + 'false' + $rs)
Check 'encl empty' (__EncL (L)) ''
Check 'decl str' ((__DecL ('a b' + $rs + $rs + 'c' + $rs) 'str') -join '|') 'a b||c'
Check 'decl int' ((__DecL ('12' + $rs + '-3' + $rs) 'int')[1]) ([long]-3)
Check 'decl bool' ((__DecL ('true' + $rs + 'false' + $rs) 'bool')[0]) $true
Check 'decl one empty item' (__DecL $rs 'str').Count 1
Check 'decl empty' (__DecL '' 'str').Count 0
Check 'json quote' (__JsonQuote ("a`"b\c`nd")) '"a\"b\\c\nd"'
Check 'json quote control' (__JsonQuote ([string][char]27)) '"\u001b"'
Check 'json surrogate pair' (__JsonUnescape '\ud83d\udc4d') ([char]::ConvertFromUtf32(0x1F44D))
Check 'sha256' (__HashSha256 'abc') 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
$tmpd = Join-Path ([IO.Path]::GetTempPath()) ('nalu-ps-' + [Guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory((Join-Path $tmpd 'p'))
[IO.File]::WriteAllText((Join-Path $tmpd 'h.txt'), 'abc')
[IO.File]::WriteAllText((Join-Path $tmpd 'p/a.txt'), 'x')
Check 'sha256 file' (__HashSha256File (Join-Path $tmpd 'h.txt')) 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
Check 'sha256 does not exist' (Fails { __HashSha256File (Join-Path $tmpd 'nao') }) $true
$script:__temps = @()
if ($null -ne (__Which 'tar')) {
    __ArchiveTarCreate (Join-Path $tmpd 'p.tgz') (Join-Path $tmpd 'p')
    __ArchiveTarExtract (Join-Path $tmpd 'p.tgz') (Join-Path $tmpd 'x')
    Check 'tar round trip' ([IO.File]::ReadAllText((Join-Path $tmpd 'x/p/a.txt'))) 'x'
}
Check 'tar folder does not exist' (Fails { __ArchiveTarCreate (Join-Path $tmpd 'q.tgz') (Join-Path $tmpd 'nao') }) $true
Compress-Archive -LiteralPath (Join-Path $tmpd 'p') -DestinationPath (Join-Path $tmpd 'p.zip')
__ArchiveZipExtract (Join-Path $tmpd 'p.zip') (Join-Path $tmpd 'z')
Check 'zip extracts' ([IO.File]::ReadAllText((Join-Path $tmpd 'z/p/a.txt'))) 'x'
Check 'zip does not exist' (Fails { __ArchiveZipExtract (Join-Path $tmpd 'nao.zip') (Join-Path $tmpd 'z2') }) $true
Check 'chmod exists' (Fails { __FsChmod (Join-Path $tmpd 'h.txt') '+x' }) $false
Check 'chmod does not exist' (Fails { __FsChmod (Join-Path $tmpd 'nao') '+x' }) $true
# --dry-run: moving a real folder INTO a temporary folder of the run is a real change, so it is only shown
$script:__temps = New-Object 'System.Collections.Generic.List[string]'
$script:__temps.Add([IO.Path]::GetFullPath((Join-Path $tmpd 'tmp')))
[void][IO.Directory]::CreateDirectory((Join-Path $tmpd 'tmp'))
$script:__dry = $true
__FsMove (Join-Path $tmpd 'p') (Join-Path $tmpd 'tmp/p')
Check 'dry-run move: source stays' ([IO.Directory]::Exists((Join-Path $tmpd 'p'))) $true
Check 'dry-run move: nothing at the destination' ([IO.Directory]::Exists((Join-Path $tmpd 'tmp/p'))) $false
[IO.File]::WriteAllText((Join-Path $tmpd 'tmp/in.txt'), 'i')
__FsMove (Join-Path $tmpd 'tmp/in.txt') (Join-Path $tmpd 'tmp/out.txt')
Check 'dry-run move inside temp runs' ([IO.File]::Exists((Join-Path $tmpd 'tmp/out.txt'))) $true
$script:__dry = $false
$script:__temps = New-Object 'System.Collections.Generic.List[string]'
__FsMove (Join-Path $tmpd 'p') (Join-Path $tmpd 'moved')
Check 'move folder' ([IO.File]::ReadAllText((Join-Path $tmpd 'moved/a.txt'))) 'x'
Remove-Item -LiteralPath $tmpd -Recurse -Force

# core
Check 'txt bool' (__Txt $true) 'true'
Check 'exitcode 300' (__ExitCode 300) 44
Check 'exitcode -1' (__ExitCode -1) 255
Check 'durstr' ((@(0, 250, 1500, 90000, 93600000, 120000) | ForEach-Object { __DurStr $_ }) -join ' ') '0s 250ms 1.5s 1m30s 1d2h 2m'
Check 'fmt' (__Fmt '{} está na porta {}' @('api', '8080')) 'api está na porta 8080'
Check 'cmp ordinal' ((__Cmp 'a' 'B') -gt 0) $true
Check 'cmp surrogate' ((__Cmp ([char]::ConvertFromUtf32(0x1F44D)) ([string][char]0xFF5A)) -gt 0) $true
$m = __Map @('a', 1, 'A', 2)
Check 'map ordinal' $m.Count 2
$c = __Copy $m
$c['a'] = 9
Check 'copy map' $m['a'] 1
$l1 = L 'só'
$l2 = __Copy $l1
Check 'copy one-item list' $l2.Count 1
Check 'copy type' ($l2 -is [System.Collections.Generic.List[object]]) $true
Check 'index out of range' (__Idx (L 1 2) 5 0L) 0L
Check 'enum parse' (__EnumParse 'prod' @('dev', 'prod') 'A') 'prod'
$ok = $false
try { __EnumParse 'Prod' @('dev', 'prod') 'A' } catch { $ok = (__ErrOf $_)['message'].Contains('is not a valid A (use: dev, prod)') }
Check 'enum parse fails' $ok $true
$e = $null
try { __Throw 'x' 7 } catch { $e = __ErrOf $_ }
Check 'throw code' $e['code'] 7L

# text
Check 'len' (__Len '👍ok') 3L
Check 'slice' (__StrSlice '👍👎x' 1 2) '👎'
Check 'slice ascii' (__StrSlice 'abcdef' 2 9223372036854775807) 'cdef'
Check 'slice negative end (JS)' (__StrSlice 'abcdef' 0 -1) 'abcde'
Check 'slice negative start (JS)' (__StrSlice '👍👎x' -2 9223372036854775807) '👎x'
Check 'slice out of range' (__StrSlice 'abc' 5 9) ''
Check 'index_of' (__StrIndexOf '👍x' 'x') 1L
Check 'split empty' (__StrSplit 'a,,b' ',').Count 3
Check 'split 1' (__StrSplit 'x' ',').Count 1
Check 'lines' ((__StrLines "a`r`nb`n") -join '|') 'a|b'
Check 'lines empty' (__StrLines '').Count 0
Check 'replace_first' (__StrReplaceFirst 'aXbX' 'X' '-') 'a-bX'
Check 'pad' (__StrPad 'é' 3 '*' $true) '**é'
Check 'to_int' (__StrToInt ' -7 ') (-7L)
foreach ($bad in @('+5', '4x', '', '1.5', '9223372036854775808')) {
    $ok = $false
    try { [void](__StrToInt $bad) } catch { $ok = $true }
    Check "to_int refuses '$bad'" $ok $true
}
Check 'chars' ((__Cps 'a👍b') -join '|') 'a|👍|b'
Check 'semver by value' (__SemverCompare '1.9' '1.10') (-1L)
Check 'semver zeros' (__SemverCompare 'v1.2' '1.2.0') 0L
Check 'semver pre' (__SemverCompare '2.0.0' '2.0.0-rc.1') 1L
Check 'semver pre parts' (__SemverCompare '1.0.0-beta.2' '1.0.0-beta.11') (-1L)
Check 'semver invalid' (Fails { __SemverCompare 'latest' '1.0' }) $true
$lc = __ListConcat (L 1 2) (L 3)
Check 'concat' ($lc -join ',') '1,2,3'
Check 'concat empty' ((__ListConcat (L) (L)).Count) 0

# lists and maps
Check 'sort text' ((__ListSort (L 'b' 'B' 'a' '_' 'A') $true) -join ' ') 'A B _ a b'
Check 'sort num' ((__ListSort (L 10L 9L 100L) $false) -join ',') '9,10,100'
Check 'sort_by stable' ((__ListSortBy (L 'b' 'B' 'a' '_' 'A') (L 'b' 'b' 'a' '_' 'a') $true) -join ' ') '_ a A b B'
Check 'slice list' ((__ListSlice (L 3 1 2) 1 9223372036854775807) -join ',') '1,2'
Check 'slice list negative (JS)' ((__ListSlice (L 3 1 2) 0 -1) -join ',') '3,1'
Check 'unique' ((__ListUnique (L 1 1 2 1 3 2)) -join ',') '1,2,3'
Check 'join bool' (__ListJoin (L $true $false) ',') 'true,false'
Check 'sum' (__ListSum (L 1L 2L 3L)) 6L
Check 'index_of' (__ListIndexOf (L 'a' 'b') 'b') 1L
Check 'keys int' ((__MapKeys (__Map @('1', 'um', '2', 'dois')) $true)[1]) 2L

# paths
Check 'name' (__PathName '/tmp/a.tar.gz') 'a.tar.gz'
Check 'ext dotfile' (__PathExt 'dir/.bashrc') ''
Check 'stem' (__PathStem 'a.tar.gz') 'a.tar'
Check 'parent' (__PathParent '/tmp/proj/x') '/tmp/proj'
Check 'parent relative' (__PathParent 'x') '.'
Check 'parent root' (__PathParent '/') '/'
Check 'pj empty' (__PJ '' 'b') 'b'
Check 'P just ~' (__P '~') ([Environment]::GetFolderPath('UserProfile'))
Check 'P ~/x' (__P '~/x') ([Environment]::GetFolderPath('UserProfile') + [IO.Path]::DirectorySeparatorChar + 'x')
Check 'P ~user stays' (__P '~foo/y') '~foo/y'
Check 'P other paths stay' (__P 'a/~') 'a/~'

# parameters
Check 'dur 1m30s' (__ParseDur '1m30s') 90000L
Check 'dur 1.5s' (__ParseDur '1.5s') 1500L
Check 'dur 500ms' (__ParseDur '500ms') 500L
Check 'dur invalid' ($null -eq (__ParseDur '5x')) $true

# commands
Check 'qarg simple' (__QArg 'abc') 'abc'
Check 'qarg space' (__QArg 'a b') '"a b"'
Check 'qarg empty' (__QArg '') '""'
Check 'qarg quotes' (__QArg 'a"b') '"a\"b"'
Check 'qarg backslash before quotes' (__QArg 'a\"b') '"a\\\"b"'
Check 'qarg trailing backslash' (__QArg 'a b\') '"a b\\"'

# time
Check 'time.format' (__TimeFormat '%Y-%m|%%' 1700000000) '2023-11|%'
$ok = $false
try { [void](__TimeFormat '%A' 0) } catch { $ok = $true }
Check 'time.format refuses %A' $ok $true

# parallel (RunspacePool): same results, order, messages and code as Bash's __par_collect
function Res($r) { return (($r['res'] | ForEach-Object { __Txt $_ }) -join ',') }
$cp0 = __Map @('l', (L (L 1 2)), 'm', (__Map @('k', 'v')))
$cp1 = __ParCopy $cp0
$cp1['l'][0].Add(3)
$cp1['m']['k'] = 'w'
Check 'parcopy deep list' $cp0['l'][0].Count 2
Check 'parcopy deep map' $cp0['m']['k'] 'v'
Check 'parcopy one-item list' ((__ParCopy (L 'x')) -is [System.Collections.Generic.List[object]]) $true
Check 'parcopy hashtable' ((__ParCopy @{ a = 1 }) -is [hashtable]) $true
function __task_tpar($o_n) {
    __Sleep ($o_n * 100)
    if ($o_n -eq 2) { __Throw 'dois falhou' 9 }
    if ($o_n -eq 4) { __Throw 'quatro falhou' 3 }
    $script:__ret = $o_n * 10 + $script:o_base
}
$script:o_base = 1L
$r = __ParRun @('__task_tpar') (L 3L 1L 2L 4L 5L) 5 @() $false
Check 'parallel fails' $r['ok'] $false
Check 'parallel results in list order' (Res $r) '31,11,,,51'
Check 'parallel errors in list order' $r['err']['message'] 'parallel: dois falhou; quatro falhou'
Check 'parallel code of the first error' $r['err']['code'] 9L
$r = __ParRun @('__task_tpar') (L) 4 @() $false
Check 'parallel empty list' (@($r['ok'], $r['res'].Count) -join ',') 'True,0'
function __task_tok { $script:__ret = 'ok' }
function __task_texit { exit 4 }
$r = __ParRun @('__task_tok', '__task_texit', '__task_tok') $null 2 @() $false
Check 'parallel exit ends only its task' $r['err']['message'] 'parallel: task 2 finished without a result'
Check 'parallel exit: code 1' $r['err']['code'] 1L
Check 'parallel exit: the others ran' (Res $r) 'ok,,ok'
function __task_tloc { $script:__ret = $o_loc + '/' + $script:o_base }
function Tloc { $o_loc = 'local'; return (__ParRun @('__task_tloc') $null 1 @('o_loc') $false) }
Check 'parallel reads the enclosing function' (Res (Tloc)) 'local/1'
$script:o_glist = L 'a'
$script:o_cnt = 1L
function __task_tglob { $script:o_glist.Add('b'); $script:o_cnt = 5L; $script:__ret = $script:o_glist.Count }
$r = __ParRun @('__task_tglob', '__task_tglob') $null 2 @() $false
Check 'parallel: each task has its own copy of the globals' (Res $r) '2,2'
Check 'parallel: global list unchanged' $script:o_glist.Count 1
Check 'parallel: global scalar unchanged' $script:o_cnt 1L
function __task_tenv {
    $script:__ret = [Environment]::GetEnvironmentVariable('NALU_PAR_T2')
    [Environment]::SetEnvironmentVariable('NALU_PAR_T', 'dentro')
    [Environment]::SetEnvironmentVariable('NALU_PAR_T2', 'mudou')
}
[Environment]::SetEnvironmentVariable('NALU_PAR_T2', 'antes')
$r = __ParRun @('__task_tenv', '__task_tenv') $null 2 @() $true
Check 'parallel: env.set does not leak (new)' ([Environment]::GetEnvironmentVariable('NALU_PAR_T')) $null
Check 'parallel: env.set does not leak (changed)' ([Environment]::GetEnvironmentVariable('NALU_PAR_T2')) 'antes'
Check 'parallel: serial tasks start from the same env' (Res $r) 'antes,antes'
[Environment]::SetEnvironmentVariable('NALU_PAR_T2', $null)
Check 'parallel: no tty inside a task' $script:__parTask $false
# without a RunspacePool (here: Get-ChildItem fails), one after another in this runspace
function Get-ChildItem { throw 'no RunspacePool' }
$r = __ParRun @('__task_tpar') (L 1L 3L 2L) 2 @() $false
$rl = Tloc
Remove-Item -Path function:Get-ChildItem
Check 'parallel fallback: results' (Res $r) '11,31,'
Check 'parallel fallback: errors' $r['err']['message'] 'parallel: dois falhou'
Check 'parallel fallback: enclosing function' (Res $rl) 'local/1'
Check 'parallel fallback: flag restored' $script:__parTask $false

if ($script:falhas -gt 0) {
    [Console]::Error.WriteLine("$($script:falhas) of $($script:total) runtime checks failed")
    exit 1
}
[Console]::Out.WriteLine("ok: $($script:total) runtime checks")
exit 0
