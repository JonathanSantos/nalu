# Nalu's PowerShell runtime: paths (text, as in Bash; `~` expanded).

# @fn __IsWin
$script:__isWin = [Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT

# @fn __P
# Expands `~` and `~/...` at the start of the path.
function __P([string]$p) {
    if ($p -eq '~') { return [Environment]::GetFolderPath('UserProfile') }
    if ($p.StartsWith('~/') -or $p.StartsWith('~\')) { return [Environment]::GetFolderPath('UserProfile') + [IO.Path]::DirectorySeparatorChar + $p.Substring(2) }
    return $p
}

# @fn __PJ
# base / child with the system separator.
function __PJ([string]$a, [string]$b) {
    if ($a.Length -eq 0) { return $b }
    $sep = [IO.Path]::DirectorySeparatorChar
    if ($a.EndsWith('/') -or $a.EndsWith([string]$sep)) { return $a + $b }
    return $a + $sep + $b
}

# @fn __PathSeps
function __PathSeps { if ($script:__isWin) { return [char[]]@('/', '\') } else { return [char[]]@('/') } }

# @fn __PathName
# @needs __P __PathSeps __IsWin
function __PathName([string]$p) {
    $p = __P $p
    $seps = __PathSeps
    $t = $p.TrimEnd($seps)
    if ($t.Length -eq 0) { if ($p.Length -gt 0) { return $p.Substring(0, 1) } else { return '' } }
    $i = $t.LastIndexOfAny($seps)
    return $t.Substring($i + 1)
}

# @fn __PathExt
# @needs __PathName
function __PathExt([string]$p) {
    $n = __PathName $p
    $i = $n.LastIndexOf('.')
    if ($i -le 0) { return '' }
    return $n.Substring($i + 1)
}

# @fn __PathStem
# @needs __PathName __PathExt
function __PathStem([string]$p) {
    $n = __PathName $p
    $e = __PathExt $p
    if ($e.Length -eq 0) { return $n }
    return $n.Substring(0, $n.Length - $e.Length - 1)
}

# @fn __PathParent
# @needs __P __PathSeps __IsWin
# dirname: "a/b" -> "a"; "a" -> "."; "/a" -> "/"; "/" -> "/"; "C:\x" -> "C:\".
function __PathParent([string]$p) {
    $p = __P $p
    $seps = __PathSeps
    $t = $p.TrimEnd($seps)
    if ($t.Length -eq 0) { if ($p.Length -gt 0) { return $p.Substring(0, 1) } else { return '.' } }
    $i = $t.LastIndexOfAny($seps)
    if ($i -lt 0) {
        if ($script:__isWin -and $t -match '^[A-Za-z]:$') { return $t + '\' }
        return '.'
    }
    $d = $t.Substring(0, $i).TrimEnd($seps)
    if ($d.Length -eq 0) { return $t.Substring(0, 1) }
    if ($script:__isWin -and $d -match '^[A-Za-z]:$') { return $d + '\' }
    return $d
}

# @fn __PathExists
# @needs __P
function __PathExists([string]$p) {
    if ($p.Length -eq 0) { return $false }
    $p = __P $p
    return [IO.File]::Exists($p) -or [IO.Directory]::Exists($p)
}

# @fn __PathIsDir
# @needs __P
function __PathIsDir([string]$p) { if ($p.Length -eq 0) { return $false }; return [IO.Directory]::Exists((__P $p)) }

# @fn __PathIsFile
# @needs __P
function __PathIsFile([string]$p) { if ($p.Length -eq 0) { return $false }; return [IO.File]::Exists((__P $p)) }

# @fn __PathAbs
# @needs __P
function __PathAbs([string]$p) {
    $p = __P $p
    if ($p.Length -eq 0) { $p = '.' }
    $f = [IO.Path]::GetFullPath([IO.Path]::Combine((Get-Location).ProviderPath, $p))
    $root = [IO.Path]::GetPathRoot($f)
    if ($f.Length -gt $root.Length) { $f = $f.TrimEnd([char[]]@('/', '\')) }
    return $f
}

# @fn __Cwd
function __Cwd { return (Get-Location).ProviderPath }

# @fn __Home
function __Home { return [Environment]::GetFolderPath('UserProfile') }

# @fn __TempBase
function __TempBase { return [IO.Path]::GetTempPath().TrimEnd([char[]]@('/', '\')) }

# @fn __AppDir
# @needs __PJ
# dirs.config/cache/data on Windows: %APPDATA%\app, %LOCALAPPDATA%\app, %APPDATA%\app (like the `dirs` crate).
function __AppDir([string]$kind, [string]$app) {
    $base = [Environment]::GetFolderPath('ApplicationData')
    if ($kind -eq 'cache') { $base = [Environment]::GetFolderPath('LocalApplicationData') }
    return __PJ $base $app
}
