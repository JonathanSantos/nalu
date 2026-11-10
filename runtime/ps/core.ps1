# Nalu's PowerShell runtime: core (errors, collections, text).
# Each block starts with `# @fn Name` and may declare `# @needs A B`. The generator includes only the blocks
# used (transitive closure), in the order they appear here. Windows PowerShell 5.1 syntax only.

# @fn __Out
function __Out([string]$s) { [Console]::Out.Write($s + "`n") }

# @fn __ErrLine
function __ErrLine([string]$s) { [Console]::Error.Write($s + "`n") }

# @fn __Throw
# Throws a Nalu error: an exception with the message, and the code in Data['nalu_code'].
function __Throw($msg, $code = 1) {
    $e = New-Object System.Exception ([string]$msg)
    $e.Data['nalu_code'] = [long]$code
    throw $e
}

# @fn __ErrOf
# Converts any caught error (ErrorRecord, Nalu or .NET exception) into { message, code }.
function __ErrOf($rec) {
    $ex = $rec
    if ($rec -is [System.Management.Automation.ErrorRecord]) { $ex = $rec.Exception }
    while (($ex -is [System.Management.Automation.MethodInvocationException] -or $ex -is [System.AggregateException] -or $ex -is [System.Reflection.TargetInvocationException]) -and $null -ne $ex.InnerException) {
        $ex = $ex.InnerException
    }
    $code = 1L
    if ($ex.Data.Contains('nalu_code')) { $code = [long]$ex.Data['nalu_code'] }
    $m = [System.Collections.Specialized.OrderedDictionary]::new([StringComparer]::Ordinal)
    $m['message'] = [string]$ex.Message
    $m['code'] = $code
    return $m
}

# @fn __ExitCode
# exit(n) with the same result as Bash: n % 256 (exit(300) = 44, exit(-1) = 255).
function __ExitCode($c) {
    $n = [long]$c % 256L
    if ($n -lt 0) { $n += 256L }
    return [int]$n
}

# @fn __Txt
# Scalar -> text: bool becomes true/false (PowerShell's [string] would give True/False).
function __Txt($v) {
    if ($v -is [bool]) { if ($v) { return 'true' } else { return 'false' } }
    return [string]$v
}

# @fn __Map
# Map/struct: OrderedDictionary with **ordinal** keys (PowerShell's @{} ignores case).
# Keys are always text; int-keyed maps convert back in keys().
function __Map($pairs) {
    $m = [System.Collections.Specialized.OrderedDictionary]::new([StringComparer]::Ordinal)
    for ($i = 0; $i -lt $pairs.Count; $i += 2) { $m[[string]$pairs[$i]] = $pairs[$i + 1] }
    return $m
}

# @fn __Copy
# Copy by value: lists and maps/structs are copied (structs deeply); scalars come back unchanged.
function __Copy($v) {
    if ($v -is [System.Collections.Specialized.OrderedDictionary]) {
        $n = [System.Collections.Specialized.OrderedDictionary]::new([StringComparer]::Ordinal)
        foreach ($k in $v.Keys) {
            $x = $v[$k]
            if ($x -is [System.Collections.Specialized.OrderedDictionary] -or $x -is [System.Collections.Generic.List[object]]) { $x = __Copy $x }
            $n[$k] = $x
        }
        return $n
    }
    if ($v -is [System.Collections.Generic.List[object]]) {
        $n = [System.Collections.Generic.List[object]]::new($v)
        for ($i = 0; $i -lt $n.Count; $i++) {
            if ($n[$i] -is [System.Collections.Specialized.OrderedDictionary]) { $n[$i] = __Copy $n[$i] }
        }
        return ,$n
    }
    return $v
}

# @fn __Idx
# list[i]: out of range returns the type's "zero" value (does not fail), as in Bash.
function __Idx($l, $i, $zero) {
    $i = [long]$i
    if ($i -ge 0 -and $i -lt $l.Count) { return $l[[int]$i] }
    return $zero
}

# @fn __Eq
# Nalu equality: text ordinal, numbers by value, structs field by field.
function __Eq($a, $b) {
    if ($a -is [string] -or $b -is [string]) { return [string]::Equals([string]$a, [string]$b) }
    if ($a -is [System.Collections.Specialized.OrderedDictionary]) {
        if (-not ($b -is [System.Collections.Specialized.OrderedDictionary]) -or $a.Count -ne $b.Count) { return $false }
        foreach ($k in $a.Keys) {
            if (-not $b.Contains($k)) { return $false }
            if (-not (__Eq $a[$k] $b[$k])) { return $false }
        }
        return $true
    }
    if ($a -is [System.Collections.Generic.List[object]]) {
        if (-not ($b -is [System.Collections.Generic.List[object]]) -or $a.Count -ne $b.Count) { return $false }
        for ($i = 0; $i -lt $a.Count; $i++) { if (-not (__Eq $a[$i] $b[$i])) { return $false } }
        return $true
    }
    return $a -eq $b
}

# @fn __Cmp
# Ordinal comparison by code point (.NET's CompareOrdinal compares UTF-16 units and gets it wrong between
# surrogates and U+E000..U+FFFF). Numbers by value.
function __Cmp($a, $b) {
    if ($a -is [string]) {
        $c = [string]::CompareOrdinal($a, $b)
        if ($c -eq 0) { return 0 }
        $n = [Math]::Min($a.Length, $b.Length)
        for ($i = 0; $i -lt $n; $i++) {
            if ($a[$i] -ne $b[$i]) {
                $x = [int]$a[$i]; $y = [int]$b[$i]
                if ([char]::IsSurrogate($a[$i])) { $x += 0x10000 }
                if ([char]::IsSurrogate($b[$i])) { $y += 0x10000 }
                if ($x -lt $y) { return -1 } else { return 1 }
            }
        }
        if ($a.Length -lt $b.Length) { return -1 } else { return 1 }
    }
    if ($a -lt $b) { return -1 }
    if ($a -gt $b) { return 1 }
    return 0
}

# @fn __Fmt
# @needs __Txt
# fmt(template, a, b...): replaces each {} with the next argument.
function __Fmt([string]$t, $items) {
    $sb = New-Object System.Text.StringBuilder
    $k = 0
    $i = 0
    while ($i -lt $t.Length) {
        if ($t[$i] -eq [char]'{' -and $i + 1 -lt $t.Length -and $t[$i + 1] -eq [char]'}' -and $k -lt $items.Count) {
            [void]$sb.Append((__Txt $items[$k]))
            $k++
            $i += 2
        } else {
            [void]$sb.Append($t[$i])
            $i++
        }
    }
    return $sb.ToString()
}

# @fn __DurStr
# duration (ms) -> text: 0s, 250ms, 1.5s, 1m30s, 1d2h.
function __DurStr($ms) {
    $ms = [long]$ms
    if ($ms -eq 0) { return '0s' }
    $neg = ''
    if ($ms -lt 0) { $neg = '-'; $ms = -$ms }
    $d = [long][Math]::Floor($ms / 86400000); $ms = $ms % 86400000
    $h = [long][Math]::Floor($ms / 3600000); $ms = $ms % 3600000
    $m = [long][Math]::Floor($ms / 60000); $ms = $ms % 60000
    $s = [long][Math]::Floor($ms / 1000); $r = $ms % 1000
    $out = $neg
    if ($d -gt 0) { $out += "${d}d" }
    if ($h -gt 0) { $out += "${h}h" }
    if ($m -gt 0) { $out += "${m}m" }
    if ($r -gt 0 -and $s -eq 0 -and $out.Length -eq $neg.Length) { return $out + "${r}ms" }
    if ($r -gt 0) {
        $frac = ([string]$r).PadLeft(3, [char]'0').TrimEnd([char]'0')
        $out += "${s}.${frac}s"
    } elseif ($s -gt 0) {
        $out += "${s}s"
    }
    return $out
}

# @fn __EnumParse
# @needs __Throw
function __EnumParse([string]$text, $cases, [string]$name) {
    foreach ($c in $cases) { if ([string]::Equals($c, $text)) { return $c } }
    __Throw ("'" + $text + "' is not a valid " + $name + " (use: " + ($cases -join ', ') + ")") 1
}

# @fn __RunDefers
# @needs __ErrOf __ErrLine
# Runs the registered defers in reverse order; failures are ignored (shown only with --verbose).
function __RunDefers($d) {
    for ($i = $d.Count - 1; $i -ge 0; $i--) {
        $f = $d[$i]
        $a = $f[1]
        try { & $f[0] @a } catch { if ($script:__verbose) { __ErrLine ('warning: defer failed: ' + (__ErrOf $_)['message']) } }
    }
    $d.Clear()
}

# @fn __Unimplemented
# @needs __Throw
function __Unimplemented([string]$what) {
    __Throw ($what + ' is not implemented yet') 70
}

# @fn __EncL
function __EncL($xs) {
    # an inner list of a nested collection as one text: each item followed by \x1e (D34)
    $sb = New-Object System.Text.StringBuilder
    foreach ($x in $xs) {
        if ($x -is [bool]) { $s = if ($x) { 'true' } else { 'false' } }
        elseif ($x -is [long] -or $x -is [int]) { $s = ([long]$x).ToString([System.Globalization.CultureInfo]::InvariantCulture) }
        else { $s = [string]$x }
        [void]$sb.Append($s).Append([char]0x1e)
    }
    return $sb.ToString()
}

# @fn __DecL
function __DecL([string]$s, [string]$kind) {
    $l = New-Object 'System.Collections.Generic.List[object]'
    if ($s.Length -eq 0) { return ,$l }
    $parts = $s.Split([char]0x1e)
    for ($i = 0; $i -lt $parts.Length - 1; $i++) {
        $p = $parts[$i]
        if ($kind -ceq 'int') { $l.Add([long]::Parse($p, [System.Globalization.CultureInfo]::InvariantCulture)) }
        elseif ($kind -ceq 'bool') { $l.Add($p -ceq 'true') }
        else { $l.Add($p) }
    }
    return ,$l
}
