# Nalu's PowerShell runtime: str methods (by code point, ordinal).

# @fn __Len
# Length in code points (an emoji outside the basic plane counts as 1, as in Bash).
function __Len([string]$s) {
    if ($s -notmatch '[\uD800-\uDFFF]') { return [long]$s.Length }
    return [long]($s.Length - [regex]::Matches($s, '[\uDC00-\uDFFF]').Count)
}

# @fn __Cps
# List of code points (each one as text).
function __Cps([string]$s) {
    $r = [System.Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $s.Length; $i++) {
        if ([char]::IsHighSurrogate($s[$i]) -and $i + 1 -lt $s.Length -and [char]::IsLowSurrogate($s[$i + 1])) {
            $r.Add($s.Substring($i, 2))
            $i++
        } else {
            $r.Add([string]$s[$i])
        }
    }
    return ,$r
}

# @fn __SliceBounds
# `slice` bounds as in JS: negative counts from the end, out of range is clamped (D21).
function __SliceBounds([long]$a, [long]$b, [long]$n) {
    if ($a -lt 0) { $a = [Math]::Max([long]0, $n + $a) }
    if ($b -lt 0) { $b = [Math]::Max([long]0, $n + $b) }
    if ($b -gt $n) { $b = $n }
    if ($a -gt $n) { $a = $n }
    return $a, $b
}

# @fn __StrSlice
# @needs __Cps __SliceBounds
function __StrSlice([string]$s, $a, $b) {
    $a = [long]$a; $b = [long]$b
    if ($s -notmatch '[\uD800-\uDFFF]') {
        $n = [long]$s.Length
        $a, $b = __SliceBounds $a $b $n
        if ($a -ge $b) { return '' }
        return $s.Substring([int]$a, [int]($b - $a))
    }
    $c = __Cps $s
    $n = [long]$c.Count
    $a, $b = __SliceBounds $a $b $n
    if ($a -ge $b) { return '' }
    return ($c.GetRange([int]$a, [int]($b - $a)) -join '')
}

# @fn __StrIndexOf
function __StrIndexOf([string]$s, [string]$sub) {
    $i = $s.IndexOf($sub, [StringComparison]::Ordinal)
    if ($i -le 0) { return [long]$i }
    return [long]($i - [regex]::Matches($s.Substring(0, $i), '[\uDC00-\uDFFF]').Count)
}

# @fn __StrSplit
function __StrSplit([string]$s, [string]$sep) {
    $r = [System.Collections.Generic.List[object]]::new()
    if ($sep.Length -eq 0) { $r.Add($s); return ,$r }
    foreach ($p in $s.Split([string[]]@($sep), [StringSplitOptions]::None)) { $r.Add($p) }
    return ,$r
}

# @fn __StrLines
# Accepts \n and \r\n; no trailing empty line.
function __StrLines([string]$s) {
    $r = [System.Collections.Generic.List[object]]::new()
    if ($s.Length -eq 0) { return ,$r }
    $parts = $s.Split([char]"`n")
    $n = $parts.Count
    if ($s.EndsWith("`n")) { $n-- }
    for ($i = 0; $i -lt $n; $i++) { $r.Add($parts[$i].TrimEnd([char]"`r")) }
    return ,$r
}

# @fn __StrReplace
function __StrReplace([string]$s, [string]$from, [string]$to) {
    if ($from.Length -eq 0) { return $s }
    return $s.Replace($from, $to)
}

# @fn __StrReplaceFirst
function __StrReplaceFirst([string]$s, [string]$from, [string]$to) {
    if ($from.Length -eq 0) { return $s }
    $i = $s.IndexOf($from, [StringComparison]::Ordinal)
    if ($i -lt 0) { return $s }
    return $s.Substring(0, $i) + $to + $s.Substring($i + $from.Length)
}

# @fn __StrRepeat
function __StrRepeat([string]$s, $n) {
    $n = [long]$n
    if ($n -le 0 -or $s.Length -eq 0) { return '' }
    return $s * [int]$n
}

# @fn __StrPad
# @needs __Len
function __StrPad([string]$s, $n, [string]$fill, [bool]$left) {
    $n = [long]$n
    if ($fill.Length -eq 0) { $fill = ' ' }
    $len = __Len $s
    $pad = ''
    while ($len -lt $n) { $pad += $fill; $len += __Len $fill }
    if ($left) { return $pad + $s }
    return $s + $pad
}

# @fn __StrToInt
# @needs __Throw
# Ignores leading/trailing spaces; rejects a + sign, decimals and values outside 64 bits.
function __StrToInt([string]$s) {
    $t = $s.Trim()
    $v = 0L
    if ($t -match '^-?[0-9]+$' -and [long]::TryParse($t, [System.Globalization.NumberStyles]::AllowLeadingSign, [System.Globalization.CultureInfo]::InvariantCulture, [ref]$v)) {
        return $v
    }
    __Throw ("'" + $s + "' is not an integer") 1
}

# @fn __StrStripPrefix
function __StrStripPrefix([string]$s, [string]$p) {
    if ($p.Length -gt 0 -and $s.StartsWith($p, [StringComparison]::Ordinal)) { return $s.Substring($p.Length) }
    return $s
}

# @fn __StrStripSuffix
function __StrStripSuffix([string]$s, [string]$p) {
    if ($p.Length -gt 0 -and $s.EndsWith($p, [StringComparison]::Ordinal)) { return $s.Substring(0, $s.Length - $p.Length) }
    return $s
}
