# Nalu: PowerShell runtime, data: json, hash and archive.

# ---- json -------------------------------------------------------------------------------------
# The same algorithm as Bash (no ConvertFrom-Json, which would lose the text of numbers like 1.50): one pass
# records the path, type and span of each value; reads return the span. Same result on every target.

# @fn __JsonWs
function __JsonWs {
    $s = $script:__jsS; $i = $script:__jsI; $n = $script:__jsN
    while ($i -lt $n) {
        $c = [int]$s[$i]
        if ($c -eq 32 -or $c -eq 9 -or $c -eq 10 -or $c -eq 13) { $i++ } else { break }
    }
    $script:__jsI = $i
}

# @fn __JsonStr
function __JsonStr {
    # __jsS[__jsI] holds a quote: advance past the closing quote
    $s = $script:__jsS; $i = $script:__jsI + 1; $n = $script:__jsN
    $stop = [char[]]@([char]'"', [char]'\')
    while ($i -lt $n) {
        $j = $s.IndexOfAny($stop, $i)
        if ($j -lt 0) { break }
        if ($s[$j] -ceq [char]'"') { $script:__jsI = $j + 1; return $true }
        $i = $j + 2
    }
    $script:__jsI = $n
    return $false
}

# @fn __JsonUnescape
function __JsonUnescape([string]$r) {
    if ($r.IndexOf([char]'\') -lt 0) { return $r }
    $sb = New-Object System.Text.StringBuilder
    $i = 0; $n = $r.Length
    while ($i -lt $n) {
        $c = $r[$i]
        if ($c -cne [char]'\' -or $i + 1 -ge $n) { [void]$sb.Append($c); $i++; continue }
        $e = [string]$r[$i + 1]
        $i += 2
        if ($e -ceq 'n') { [void]$sb.Append([char]10) }
        elseif ($e -ceq 't') { [void]$sb.Append([char]9) }
        elseif ($e -ceq 'r') { [void]$sb.Append([char]13) }
        elseif ($e -ceq 'b') { [void]$sb.Append([char]8) }
        elseif ($e -ceq 'f') { [void]$sb.Append([char]12) }
        elseif ($e -ceq 'u') {
            if ($i + 4 -le $n -and $r.Substring($i, 4) -cmatch '^[0-9a-fA-F]{4}$') {
                $cp = [Convert]::ToInt32($r.Substring($i, 4), 16)
                $i += 4
                if ($cp -ne 0) { [void]$sb.Append([char]$cp) }
            } else { [void]$sb.Append('\u') }
        }
        else { [void]$sb.Append($e) }
    }
    return $sb.ToString()
}

# @fn __JsonVal
# @needs __JsonWs __JsonStr __JsonUnescape
function __JsonVal([string]$path) {
    __JsonWs
    $s = $script:__jsS; $n = $script:__jsN
    if ($script:__jsI -ge $n) { return $false }
    $k = $script:__jsP.Count
    $script:__jsP.Add($path); $script:__jsA.Add($script:__jsI); $script:__jsB.Add(0); $script:__jsT.Add([char]'?')
    $c = $s[$script:__jsI]
    $sep = [string][char]0x1f
    if ($c -ceq [char]'"') {
        $script:__jsT[$k] = [char]'s'
        if (-not (__JsonStr)) { return $false }
    } elseif ($c -ceq [char]'{') {
        $script:__jsT[$k] = [char]'o'
        $script:__jsI++
        __JsonWs
        if ($script:__jsI -lt $n -and $s[$script:__jsI] -ceq [char]'}') { $script:__jsI++ }
        else {
            while ($true) {
                __JsonWs
                if ($script:__jsI -ge $n -or $s[$script:__jsI] -cne [char]'"') { return $false }
                $a = $script:__jsI
                if (-not (__JsonStr)) { return $false }
                $key = __JsonUnescape $s.Substring($a + 1, $script:__jsI - $a - 2)
                __JsonWs
                if ($script:__jsI -ge $n -or $s[$script:__jsI] -cne [char]':') { return $false }
                $script:__jsI++
                if (-not (__JsonVal ($path + $sep + $key))) { return $false }
                __JsonWs
                if ($script:__jsI -ge $n) { return $false }
                $c = $s[$script:__jsI]; $script:__jsI++
                if ($c -ceq [char]',') { continue }
                if ($c -ceq [char]'}') { break }
                return $false
            }
        }
    } elseif ($c -ceq [char]'[') {
        $script:__jsT[$k] = [char]'a'
        $script:__jsI++
        __JsonWs
        if ($script:__jsI -lt $n -and $s[$script:__jsI] -ceq [char]']') { $script:__jsI++ }
        else {
            $x = 0
            while ($true) {
                if (-not (__JsonVal ($path + $sep + $x))) { return $false }
                $x++
                __JsonWs
                if ($script:__jsI -ge $n) { return $false }
                $c = $s[$script:__jsI]; $script:__jsI++
                if ($c -ceq [char]',') { continue }
                if ($c -ceq [char]']') { break }
                return $false
            }
        }
    } else {
        $a = $script:__jsI
        $j = $s.IndexOfAny([char[]]@([char]',', [char]'}', [char]']', [char]' ', [char]9, [char]10, [char]13), $a)
        if ($j -lt 0) { $j = $n }
        $tok = $s.Substring($a, $j - $a)
        $script:__jsI = $j
        if ($tok -ceq 'true' -or $tok -ceq 'false') { $script:__jsT[$k] = [char]'b' }
        elseif ($tok -ceq 'null') { $script:__jsT[$k] = [char]'z' }
        elseif ($tok -cmatch '^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?\z') { $script:__jsT[$k] = [char]'n' }
        else { return $false }
    }
    $script:__jsB[$k] = $script:__jsI
    return $true
}

# @fn __JsonParse
# @needs __Throw __JsonVal __JsonWs
function __JsonParse([string]$s) {
    # strict mode: the cache only exists after the first read
    if ((Test-Path variable:script:__jsOk) -and $script:__jsOk -and [string]::Equals($script:__jsSrc, $s, [StringComparison]::Ordinal)) { return }
    $script:__jsOk = $false
    $script:__jsP = New-Object 'System.Collections.Generic.List[string]'
    $script:__jsT = New-Object 'System.Collections.Generic.List[char]'
    $script:__jsA = New-Object 'System.Collections.Generic.List[int]'
    $script:__jsB = New-Object 'System.Collections.Generic.List[int]'
    $script:__jsS = $s; $script:__jsN = $s.Length; $script:__jsI = 0
    $ok = __JsonVal ''
    if ($ok) { __JsonWs }
    if (-not $ok -or $script:__jsI -ne $script:__jsN) { __Throw 'json: invalid JSON' 1 }
    $script:__jsSrc = $s
    $script:__jsOk = $true
}

# @fn __JsonFind
# @needs __Throw __JsonParse
function __JsonFind([string]$text, [string]$path) {
    __JsonParse $text
    $p = $path.Replace('[', '.').Replace(']', '')
    if ($p.StartsWith('.')) { $p = $p.Substring(1) }
    $w = ''
    if ($p.Length -gt 0) { foreach ($seg in $p.Split([char]'.')) { $w += [string][char]0x1f + $seg } }
    $k = $script:__jsP.IndexOf($w)
    if ($k -lt 0) { __Throw ("json: '" + $path + "' does not exist") 1 }
    return $k
}

# @fn __JsonItem
# @needs __JsonUnescape
function __JsonItem([int]$k) {
    $raw = $script:__jsS.Substring($script:__jsA[$k], $script:__jsB[$k] - $script:__jsA[$k])
    if ($script:__jsT[$k] -ceq [char]'s') { return (__JsonUnescape $raw.Substring(1, $raw.Length - 2)) }
    return $raw
}

# @fn __JsonGet
# @needs __Throw __JsonFind __JsonItem
function __JsonGet([string]$text, [string]$path) {
    $k = __JsonFind $text $path
    if ($script:__jsT[$k] -ceq [char]'z') { __Throw ("json: '" + $path + "' is null") 1 }
    return (__JsonItem $k)
}

# @fn __JsonChildren
# @needs __Throw __JsonFind __JsonItem
function __JsonChildren([string]$text, [string]$path, [string]$kind, [string]$what) {
    $k = __JsonFind $text $path
    if ([string]$script:__jsT[$k] -cne $kind) {
        if ($kind -ceq 'a') { __Throw ("json: '" + $path + "' is not a list") 1 }
        __Throw ("json: '" + $path + "' is not an object") 1
    }
    $pre = $script:__jsP[$k] + [string][char]0x1f
    $r = New-Object 'System.Collections.Generic.List[object]'
    for ($x = $k + 1; $x -lt $script:__jsP.Count -and $script:__jsA[$x] -lt $script:__jsB[$k]; $x++) {
        $px = $script:__jsP[$x]
        if (-not $px.StartsWith($pre, [StringComparison]::Ordinal)) { continue }
        $rest = $px.Substring($pre.Length)
        if ($rest.IndexOf([char]0x1f) -ge 0) { continue }
        if ($what -ceq 'key') { $r.Add($rest) } else { $r.Add((__JsonItem $x)) }
    }
    return ,$r
}

# @fn __JsonHas
# @needs __JsonFind
function __JsonHas([string]$text, [string]$path) {
    try { $k = __JsonFind $text $path } catch { return $false }
    return ($script:__jsT[$k] -cne [char]'z')
}

# @fn __JsonQuote
function __JsonQuote([string]$s) {
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.Append('"')
    foreach ($c in $s.ToCharArray()) {
        $x = [int]$c
        if ($x -eq 34) { [void]$sb.Append('\"') }
        elseif ($x -eq 92) { [void]$sb.Append('\\') }
        elseif ($x -eq 10) { [void]$sb.Append('\n') }
        elseif ($x -eq 13) { [void]$sb.Append('\r') }
        elseif ($x -eq 9) { [void]$sb.Append('\t') }
        elseif ($x -eq 8) { [void]$sb.Append('\b') }
        elseif ($x -eq 12) { [void]$sb.Append('\f') }
        elseif ($x -lt 32) { [void]$sb.Append(('\u{0:x4}' -f $x)) }
        else { [void]$sb.Append($c) }
    }
    [void]$sb.Append('"')
    return $sb.ToString()
}

# ---- hash -------------------------------------------------------------------------------------

# @fn __Hex
function __Hex([byte[]]$b) { return ([BitConverter]::ToString($b).Replace('-', '')).ToLowerInvariant() }

# @fn __HashSha256
# @needs __Hex
function __HashSha256([string]$s) {
    $h = [System.Security.Cryptography.SHA256]::Create()
    try { return (__Hex $h.ComputeHash((New-Object System.Text.UTF8Encoding $false).GetBytes($s))) } finally { $h.Dispose() }
}

# @fn __HashSha256File
# @needs __Hex __Full __Throw
function __HashSha256File([string]$p) {
    $f = __Full $p
    if (-not [IO.File]::Exists($f)) { __Throw ("hash.sha256_file: '" + $p + "' does not exist") 1 }
    $h = [System.Security.Cryptography.SHA256]::Create()
    $st = [IO.File]::OpenRead($f)
    try { return (__Hex $h.ComputeHash($st)) } finally { $st.Dispose(); $h.Dispose() }
}

# ---- archive ----------------------------------------------------------------------------------

# @fn __Tar
# @needs __Which __StartProc __Throw
# The tar.exe shipped with Windows 10 (1803+) and 11 is bsdtar: same options as on Unix.
function __Tar([string]$what, $words) {
    $exe = __Which 'tar'
    if ($null -eq $exe) { __Throw ($what + ': could not find tar (Windows 10 1803+ includes it)') 1 }
    $p = __StartProc $exe $words $false $true $true
    $null = $p.StandardOutput.ReadToEnd()
    $null = $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    return $p.ExitCode
}

# @fn __ArchiveTarCreate
# @needs __Full __DryW __Throw __Tar
function __ArchiveTarCreate([string]$file, [string]$dir) {
    $d = (__Full $dir).TrimEnd([char]'\', [char]'/')
    if (__DryW ('archive.tar_create ' + $file + ' ' + $dir) $file) { return }
    if (-not [IO.Directory]::Exists($d)) { __Throw ("archive.tar_create: folder '" + $dir + "' does not exist") 1 }
    $parent = [IO.Path]::GetDirectoryName($d)
    if ([string]::IsNullOrEmpty($parent)) { $parent = $d }
    $base = [IO.Path]::GetFileName($d)
    $code = __Tar 'archive.tar_create' @('tar', '-czf', (__Full $file), '-C', $parent, '--', $base)
    if ($code -ne 0) { __Throw ("archive.tar_create: could not create '" + $file + "'") 1 }
}

# @fn __ArchiveTarExtract
# @needs __Full __DryW __Throw __Tar
function __ArchiveTarExtract([string]$file, [string]$to) {
    $f = __Full $file
    if (__DryW ('archive.tar_extract ' + $file + ' ' + $to) $to) { return }
    if (-not [IO.File]::Exists($f)) { __Throw ("archive.tar_extract: '" + $file + "' does not exist") 1 }
    $t = __Full $to
    [void][IO.Directory]::CreateDirectory($t)
    $code = __Tar 'archive.tar_extract' @('tar', '-xzf', $f, '-C', $t)
    if ($code -ne 0) { __Throw ("archive.tar_extract: could not extract '" + $file + "'") 1 }
}

# @fn __ArchiveZipCreate
# @needs __Full __DryW __Throw
# .NET's ZipFile instead of Compress-Archive, which in 5.1 writes entry names with `\` (broken on Unix).
function __ArchiveZipCreate([string]$file, [string]$dir) {
    $d = (__Full $dir).TrimEnd([char]'\', [char]'/')
    if (__DryW ('archive.zip_create ' + $file + ' ' + $dir) $file) { return }
    if (-not [IO.Directory]::Exists($d)) { __Throw ("archive.zip_create: folder '" + $dir + "' does not exist") 1 }
    $f = __Full $file
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction Stop
        if ([IO.File]::Exists($f)) { [IO.File]::Delete($f) }
        [IO.Compression.ZipFile]::CreateFromDirectory($d, $f, [IO.Compression.CompressionLevel]::Optimal, $true)
    } catch { __Throw ("archive.zip_create: could not create '" + $file + "'") 1 }
}

# @fn __ArchiveZipExtract
# @needs __Full __DryW __Throw
function __ArchiveZipExtract([string]$file, [string]$to) {
    $f = __Full $file
    if (__DryW ('archive.zip_extract ' + $file + ' ' + $to) $to) { return }
    if (-not [IO.File]::Exists($f)) { __Throw ("archive.zip_extract: '" + $file + "' does not exist") 1 }
    $t = __Full $to
    $ProgressPreference = 'SilentlyContinue'
    try {
        [void][IO.Directory]::CreateDirectory($t)
        Expand-Archive -LiteralPath $f -DestinationPath $t -Force -ErrorAction Stop
    } catch { __Throw ("archive.zip_extract: could not extract '" + $file + "'") 1 }
}

# @fn __FsChmod
# @needs __Full __DryW __Throw
# Windows has no permission bits: checks the path (as on Unix) and does nothing.
function __FsChmod([string]$p, [string]$mode) {
    $f = __Full $p
    if (__DryW ('fs.chmod ' + $mode + ' ' + $p) $p) { return }
    if (-not ([IO.File]::Exists($f) -or [IO.Directory]::Exists($f))) { __Throw ("fs.chmod: '" + $p + "' does not exist") 1 }
}

# ---- regex (D32): the compiler translated the pattern to .NET syntax ----

# @fn __ReFind
# @needs __Throw
function __ReFind([string]$t, [string]$re, [string]$src) {
    $m = [regex]::Match($t, $re)
    if (-not $m.Success) { __Throw ("regex: no match for '" + $src + "'") 1 }
    return $m.Value
}

# @fn __ReGroups
# @needs __Throw
function __ReGroups([string]$t, [string]$re, [string]$src) {
    $m = [regex]::Match($t, $re)
    if (-not $m.Success) { __Throw ("regex: no match for '" + $src + "'") 1 }
    $r = New-Object 'System.Collections.Generic.List[object]'
    for ($i = 1; $i -lt $m.Groups.Count; $i++) { $r.Add($m.Groups[$i].Value) }
    return ,$r
}

# @fn __ReAll
function __ReAll([string]$t, [string]$re) {
    $r = New-Object 'System.Collections.Generic.List[object]'
    foreach ($m in [regex]::Matches($t, $re)) { $r.Add($m.Value) }
    return ,$r
}

# @fn __ReReplace
function __ReReplace([string]$t, [string]$re, [string]$new) {
    return [regex]::Replace($t, $re, $new.Replace('$', '$$'))
}

# ---- encoding ----

# @fn __B64Enc
function __B64Enc([string]$t) {
    return [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($t))
}

# @fn __B64Dec
# @needs __Throw
function __B64Dec([string]$t) {
    # spaces and line breaks ignored; URL-safe alphabet and missing padding accepted (same rules as Bash)
    $s = ($t -replace '[ \t\r\n]', '').Replace('-', '+').Replace('_', '/')
    $d = $s
    if ($d.EndsWith('=')) { $d = $d.Substring(0, $d.Length - 1) }
    if ($d.EndsWith('=')) { $d = $d.Substring(0, $d.Length - 1) }
    if ($d -cnotmatch '\A[A-Za-z0-9+/]*\z' -or ($d.Length % 4) -eq 1) { __Throw 'encoding: the text is not Base64' 1 }
    while (($d.Length % 4) -ne 0) { $d += '=' }
    $bytes = [Convert]::FromBase64String($d)
    if ([Array]::IndexOf($bytes, [byte]0) -ge 0) { __Throw 'encoding: the decoded bytes are not UTF-8 text' 1 }
    try { return (New-Object System.Text.UTF8Encoding $false, $true).GetString($bytes) }
    catch { __Throw 'encoding: the decoded bytes are not UTF-8 text' 1 }
}

# @fn __SemverSplit
# @needs __Throw
# A version as its parts: c = the numbers, p = the pre-release ($null if none). Same rules as Bash.
function __SemverSplit([string]$v) {
    $s = $v
    if ($s.Length -gt 0 -and ($s[0] -ceq 'v' -or $s[0] -ceq 'V')) { $s = $s.Substring(1) }
    $i = $s.IndexOf('+')
    if ($i -ge 0) { $s = $s.Substring(0, $i) }
    $pre = $null
    $i = $s.IndexOf('-')
    if ($i -ge 0) { $pre = $s.Substring($i + 1); $s = $s.Substring(0, $i) }
    $ok = $s -cmatch '\A[0-9]{1,18}(\.[0-9]{1,18})*\z'
    if ($ok -and $null -ne $pre) {
        $ok = $pre -cmatch '\A[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*\z'
        if ($ok) { foreach ($x in $pre.Split('.')) { if ($x -cmatch '\A[0-9]{19,}\z') { $ok = $false } } }
    }
    if (-not $ok) { __Throw ("semver: '" + $v + "' is not a version") 1 }
    $p = $null
    if ($null -ne $pre) { $p = $pre.Split('.') }
    return @{ c = $s.Split('.'); p = $p }
}

# @fn __SemverCompare
# @needs __SemverSplit
function __SemverCompare([string]$a, [string]$b) {
    $x = __SemverSplit $a
    $y = __SemverSplit $b
    $n = [Math]::Max($x.c.Length, $y.c.Length)
    for ($i = 0; $i -lt $n; $i++) {
        $p = 0L; $q = 0L
        if ($i -lt $x.c.Length) { $p = [long]::Parse($x.c[$i], [Globalization.CultureInfo]::InvariantCulture) }
        if ($i -lt $y.c.Length) { $q = [long]::Parse($y.c[$i], [Globalization.CultureInfo]::InvariantCulture) }
        if ($p -lt $q) { return -1L }
        if ($p -gt $q) { return 1L }
    }
    # same numbers: a pre-release is older than the release; two pre-releases compare part by part
    if ($null -eq $x.p -and $null -eq $y.p) { return 0L }
    if ($null -eq $x.p) { return 1L }
    if ($null -eq $y.p) { return -1L }
    for ($i = 0; ; $i++) {
        if ($i -ge $x.p.Length -and $i -ge $y.p.Length) { return 0L }
        if ($i -ge $x.p.Length) { return -1L }
        if ($i -ge $y.p.Length) { return 1L }
        $s = [string]$x.p[$i]; $t = [string]$y.p[$i]
        $sn = $s -cmatch '\A[0-9]+\z'; $tn = $t -cmatch '\A[0-9]+\z'
        if ($sn -and $tn) {
            $p = [long]::Parse($s, [Globalization.CultureInfo]::InvariantCulture)
            $q = [long]::Parse($t, [Globalization.CultureInfo]::InvariantCulture)
            if ($p -lt $q) { return -1L }
            if ($p -gt $q) { return 1L }
        } elseif ($sn) { return -1L }
        elseif ($tn) { return 1L }
        else {
            $c = [string]::CompareOrdinal($s, $t)
            if ($c -lt 0) { return -1L }
            if ($c -gt 0) { return 1L }
        }
    }
}
