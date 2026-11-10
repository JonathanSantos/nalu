# Nalu's PowerShell runtime: reading the script's parameters.
# Hand-written $args reader (not PowerShell's param()): accepts --x v, --x=v, bool flags, lists a,b, `--`.

# @fn __UsageErr
# @needs __ErrLine
function __UsageErr([string]$msg) {
    __ErrLine ('error: ' + $msg)
    __ErrLine 'run with --help to see the options'
    exit 2
}

# @fn __ParseDur
# Duration in the literal grammar (1m30s, 500ms, 1.5s, 2h, 7d); $null if invalid.
function __ParseDur([string]$s) {
    $m = [regex]::Match($s, '^(?:(\d+)d)?(?:(\d+)h)?(?:(\d+)m(?!s))?(?:(\d+)(?:\.(\d{1,3}))?s)?(?:(\d+)ms)?$')
    if (-not $m.Success -or $s.Length -eq 0) { return $null }
    $ms = 0L
    if ($m.Groups[1].Success) { $ms += [long]$m.Groups[1].Value * 86400000L }
    if ($m.Groups[2].Success) { $ms += [long]$m.Groups[2].Value * 3600000L }
    if ($m.Groups[3].Success) { $ms += [long]$m.Groups[3].Value * 60000L }
    if ($m.Groups[4].Success) { $ms += [long]$m.Groups[4].Value * 1000L }
    if ($m.Groups[5].Success) { $ms += [long]($m.Groups[5].Value.PadRight(3, [char]'0')) }
    if ($m.Groups[6].Success) { $ms += [long]$m.Groups[6].Value }
    return $ms
}

# @fn __ParamVal
# @needs __UsageErr __ParseDur
function __ParamVal([string]$flag, [string]$kind, [string]$v, $cases) {
    switch ($kind) {
        'int' {
            $n = 0L
            if ($v -match '^-?[0-9]+$' -and [long]::TryParse($v, [System.Globalization.NumberStyles]::AllowLeadingSign, [System.Globalization.CultureInfo]::InvariantCulture, [ref]$n)) { return $n }
            __UsageErr ($flag + " must be an integer (got '" + $v + "')")
        }
        'duration' {
            $d = __ParseDur $v
            if ($null -eq $d) { __UsageErr ($flag + " must be a duration like 30s, 1m30s or 500ms (got '" + $v + "')") }
            return $d
        }
        'enum' {
            foreach ($c in $cases) { if ([string]::Equals($c, $v)) { return $v } }
            __UsageErr ($flag + ' must be one of: ' + ($cases -join ', ') + " (got '" + $v + "')")
        }
        'bool' {
            if ($v -eq 'true') { return $true }
            if ($v -eq 'false') { return $false }
            __UsageErr ($flag + " takes true or false (got '" + $v + "')")
        }
        'list_str' {
            $l = [System.Collections.Generic.List[object]]::new()
            if ($v.Length -gt 0) { foreach ($x in $v.Split([char]',')) { $l.Add($x) } }
            return ,$l
        }
        'list_int' {
            $l = [System.Collections.Generic.List[object]]::new()
            if ($v.Length -gt 0) { foreach ($x in $v.Split([char]',')) { $l.Add((__ParamVal $flag 'int' $x.Trim() $null)) } }
            return ,$l
        }
        default { return $v }
    }
}

# @fn __ParseArgs
# @needs __UsageErr __ParamVal __Out
# $spec: list of @{ f = '--flag'; k = type; c = enum cases }. Returns a dictionary flag -> value and
# '' -> list of positionals. --help prints the usage and exits with 0.
function __ParseArgs($argv, $spec, [string]$usage) {
    $byFlag = @{}
    foreach ($s in $spec) { $byFlag[$s.f] = $s }
    $res = [System.Collections.Specialized.OrderedDictionary]::new([StringComparer]::Ordinal)
    $pos = [System.Collections.Generic.List[object]]::new()
    foreach ($a in $argv) {
        if ([string]$a -eq '--') { break }
        if ([string]$a -eq '-h' -or [string]$a -eq '--help') { __Out $usage; exit 0 }
    }
    $i = 0
    while ($i -lt $argv.Count) {
        $a = [string]$argv[$i]
        $i++
        if ($a -eq '--') {
            while ($i -lt $argv.Count) { $pos.Add([string]$argv[$i]); $i++ }
            break
        }
        if (-not $a.StartsWith('-') -or $a -eq '-') { $pos.Add($a); continue }
        $name = $a
        $val = $null
        $eq = $a.IndexOf('=')
        if ($a.StartsWith('--') -and $eq -gt 2) { $name = $a.Substring(0, $eq); $val = $a.Substring($eq + 1) }
        if ($name -ceq '--dry-run' -or $name -ceq '--verbose') {
            $on = $true
            if ($null -ne $val) { $on = __ParamVal $name 'bool' $val $null }
            if ($name -ceq '--dry-run') { $script:__dry = $on } else { $script:__verbose = $on }
            continue
        }
        if (-not $byFlag.ContainsKey($name) -or -not ($byFlag[$name].f -ceq $name)) { __UsageErr ('unknown option: ' + $name) }
        $s = $byFlag[$name]
        if ($s.k -eq 'bool') {
            if ($null -eq $val) { $res[$name] = $true } else { $res[$name] = __ParamVal $name 'bool' $val $null }
            continue
        }
        if ($null -eq $val) {
            if ($i -ge $argv.Count) { __UsageErr ($name + ' needs a value') }
            $val = [string]$argv[$i]
            $i++
        }
        $res[$name] = __ParamVal $name ([string]$s.k) $val $s.c
    }
    $res[''] = $pos
    return $res
}
