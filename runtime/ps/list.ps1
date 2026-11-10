# Nalu's PowerShell runtime: lists (System.Collections.Generic.List[object]) and maps.
# Every function that returns a list uses `return ,$l` so PowerShell does not unroll 1-item lists.

# @fn __ListPop
# @needs __Throw
function __ListPop($l) {
    if ($l.Count -eq 0) { __Throw 'pop: the list is empty' 1 }
    $x = $l[$l.Count - 1]
    $l.RemoveAt($l.Count - 1)
    return $x
}

# @fn __ListFirst
# @needs __Throw
function __ListFirst($l) {
    if ($l.Count -eq 0) { __Throw 'first: the list is empty' 1 }
    return $l[0]
}

# @fn __ListLast
# @needs __Throw
function __ListLast($l) {
    if ($l.Count -eq 0) { __Throw 'last: the list is empty' 1 }
    return $l[$l.Count - 1]
}

# @fn __ListIndexOf
# @needs __Eq
function __ListIndexOf($l, $x) {
    for ($i = 0; $i -lt $l.Count; $i++) { if (__Eq $l[$i] $x) { return [long]$i } }
    return -1L
}

# @fn __ListJoin
# @needs __Txt
function __ListJoin($l, [string]$sep) {
    $parts = New-Object 'System.Collections.Generic.List[string]'
    foreach ($x in $l) { $parts.Add((__Txt $x)) }
    return [string]::Join($sep, $parts)
}

# @fn __ListSum
function __ListSum($l) {
    $t = 0L
    foreach ($x in $l) { $t += [long]$x }
    return $t
}

# @fn __ListSort
# Text in ordinal order; numbers by value. Returns a new list.
function __ListSort($l, [bool]$text) {
    $r = [System.Collections.Generic.List[object]]::new()
    if ($text) {
        $a = [string[]]$l.ToArray()
        [Array]::Sort($a, [StringComparer]::Ordinal)
    } else {
        $a = [long[]]$l.ToArray()
        [Array]::Sort($a)
    }
    foreach ($x in $a) { $r.Add($x) }
    return ,$r
}

# @fn __ListSortBy
# Stable sort by the precomputed keys (LINQ OrderBy is stable).
function __ListSortBy($l, $keys, [bool]$text) {
    $idx = [int[]](0..($l.Count - 1))
    if ($l.Count -eq 0) { $idx = [int[]]@() }
    if ($text) {
        $ks = [string[]]$keys.ToArray()
        $ord = [System.Linq.Enumerable]::OrderBy($idx, [Func[int, string]] { param($i) $ks[$i] }, [StringComparer]::Ordinal)
    } else {
        $ks = [long[]]$keys.ToArray()
        $ord = [System.Linq.Enumerable]::OrderBy($idx, [Func[int, long]] { param($i) $ks[$i] })
    }
    $r = [System.Collections.Generic.List[object]]::new()
    foreach ($i in $ord) { $r.Add($l[$i]) }
    return ,$r
}

# @fn __ListReverse
function __ListReverse($l) {
    $r = [System.Collections.Generic.List[object]]::new($l)
    $r.Reverse()
    return ,$r
}

# @fn __ListConcat
function __ListConcat($a, $b) {
    $r = [System.Collections.Generic.List[object]]::new($a)
    $r.AddRange($b)
    return ,$r
}

# @fn __ListSlice
# @needs __SliceBounds
function __ListSlice($l, $a, $b) {
    $n = [long]$l.Count
    $a, $b = __SliceBounds ([long]$a) ([long]$b) $n
    $r = [System.Collections.Generic.List[object]]::new()
    if ($a -lt $b) { $r.AddRange($l.GetRange([int]$a, [int]($b - $a))) }
    return ,$r
}

# @fn __ListUnique
# @needs __Eq
function __ListUnique($l) {
    $r = [System.Collections.Generic.List[object]]::new()
    foreach ($x in $l) {
        $dup = $false
        foreach ($y in $r) { if (__Eq $x $y) { $dup = $true; break } }
        if (-not $dup) { $r.Add($x) }
    }
    return ,$r
}

# @fn __MapGet
# @needs __Throw
function __MapGet($m, $k) {
    $k = [string]$k
    if (-not $m.Contains($k)) { __Throw ("key '" + $k + "' does not exist") 1 }
    return $m[$k]
}

# @fn __MapKeys
function __MapKeys($m, [bool]$int) {
    $r = [System.Collections.Generic.List[object]]::new()
    foreach ($k in $m.Keys) { if ($int) { $r.Add([long]$k) } else { $r.Add($k) } }
    return ,$r
}

# @fn __MapValues
function __MapValues($m) {
    $r = [System.Collections.Generic.List[object]]::new()
    foreach ($v in $m.Values) { $r.Add($v) }
    return ,$r
}
