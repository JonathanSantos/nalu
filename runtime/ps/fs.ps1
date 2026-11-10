# Nalu's PowerShell runtime: fs.* via [IO.File] (UTF-8 without BOM, \n), dry-run and rm guards.

# @fn __Temps
$script:__temps = New-Object 'System.Collections.Generic.List[string]'

# @fn __Full
# @needs __P
function __Full([string]$p) {
    $p = __P $p
    if ($p.Length -eq 0) { return '' }
    return [IO.Path]::GetFullPath([IO.Path]::Combine((Get-Location).ProviderPath, $p))
}

# @fn __DryW
# @needs __Full __Temps __ErrLine
# In --dry-run, shows the write and returns $true (skip), except inside this run's temporaries.
function __DryW([string]$what, [string]$p, [string]$p2 = '') {
    if (-not $script:__dry) { return $false }
    # runs only when every path is inside a temporary folder of this run (like `__dry_skip` in Bash)
    $inside = $true
    foreach ($q in @($p, $p2)) {
        if ($q -eq '') { continue }
        $f = __Full $q
        $hit = $false
        foreach ($t in $script:__temps) {
            if ($f -eq $t -or $f.StartsWith($t + [IO.Path]::DirectorySeparatorChar)) { $hit = $true; break }
        }
        if (-not $hit) { $inside = $false }
    }
    if ($inside) { return $false }
    __ErrLine ('[dry-run] ' + $what)
    return $true
}

# @fn __FsErr
# @needs __Throw
# The same messages as the Bash runtime: "<op>: '<p>' does not exist" or "<op>: could not <verb> '<p>'".
function __FsErr([string]$op, [string]$verb, [string]$p, $rec) {
    $m = $rec
    if ($rec -is [System.Management.Automation.ErrorRecord]) { $m = $rec.Exception }
    while ($m -is [System.Management.Automation.MethodInvocationException] -and $null -ne $m.InnerException) { $m = $m.InnerException }
    if ($m -is [System.IO.FileNotFoundException] -or $m -is [System.IO.DirectoryNotFoundException]) { __Throw ($op + ": '" + $p + "' does not exist") 1 }
    if ($m -is [System.UnauthorizedAccessException]) { __Throw ($op + ': could not ' + $verb + " '" + $p + "' (permission denied)") 1 }
    __Throw ($op + ': could not ' + $verb + " '" + $p + "'") 1
}

# @fn __Utf8
$script:__utf8 = New-Object System.Text.UTF8Encoding $false

# @fn __FsRead
# @needs __P __FsErr __Utf8 __Throw
# Reads bytes and decodes as UTF-8 with no detection (no UTF-16 via BOM); strips a UTF-8 BOM if present.
function __FsRead([string]$p) {
    if ([IO.Directory]::Exists((__P $p))) { __Throw ("fs.read: '" + $p + "' is a folder") 1 }
    try { $b = [IO.File]::ReadAllBytes((__P $p)) } catch { __FsErr 'fs.read' 'read' $p $_ }
    $off = 0
    if ($b.Length -ge 3 -and $b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF) { $off = 3 }
    return $script:__utf8.GetString($b, $off, $b.Length - $off)
}

# @fn __FsWrite
# @needs __P __FsErr __DryW __Utf8
function __FsWrite([string]$p, [string]$text, [bool]$append) {
    $what = 'fs.write'
    if ($append) { $what = 'fs.append' }
    if (__DryW ($what + ' ' + $p) $p) { return }
    try {
        if ($append) { [IO.File]::AppendAllText((__P $p), $text, $script:__utf8) }
        else { [IO.File]::WriteAllText((__P $p), $text, $script:__utf8) }
    } catch { __FsErr $what 'write' $p $_ }
}

# @fn __FsRm
# @needs __P __Full __Throw __FsErr __DryW
# Recursive; does not fail if missing; refuses "", /, ., .., ~, the home folder and the current folder.
function __FsRm([string]$p) {
    $x = $p.Trim()
    if ($x -eq '' -or $x -eq '/' -or $x -eq '\' -or $x -eq '.' -or $x -eq '..' -or $x -eq '~' -or $x -eq './' -or $x -eq '.\' -or $x -match '^[A-Za-z]:[\\/]?$') {
        __Throw ("fs.rm refused to delete '" + $p + "'") 1
    }
    $f = (__Full $p).TrimEnd([char[]]@('/', '\'))
    $hm = [Environment]::GetFolderPath('UserProfile').TrimEnd([char[]]@('/', '\'))
    $cwd = (Get-Location).ProviderPath.TrimEnd([char[]]@('/', '\'))
    if ($f -eq $hm -or $f -eq $cwd -or $f.Length -eq 0 -or $f -eq [IO.Path]::GetPathRoot($f).TrimEnd([char[]]@('/', '\'))) {
        __Throw ("fs.rm refused to delete '" + $p + "'") 1
    }
    if ($cwd.StartsWith($f + [IO.Path]::DirectorySeparatorChar)) { __Throw ("fs.rm refused to delete '" + $p + "' (it contains the current folder)") 1 }
    if (__DryW ('fs.rm ' + $p) $p) { return }
    try {
        if ([IO.Directory]::Exists($f)) {
            $attr = [IO.File]::GetAttributes($f)
            if (($attr -band [IO.FileAttributes]::ReparsePoint) -ne 0) { [IO.Directory]::Delete($f, $false) }
            else { [IO.Directory]::Delete($f, $true) }
        } elseif ([IO.File]::Exists($f)) {
            [IO.File]::Delete($f)
        }
    } catch { __FsErr 'fs.rm' 'delete' $p $_ }
}

# @fn __FsMkdir
# @needs __P __FsErr __DryW
function __FsMkdir([string]$p) {
    if (__DryW ('fs.mkdir ' + $p) $p) { return }
    try { [void][IO.Directory]::CreateDirectory((__P $p)) } catch { __FsErr 'fs.mkdir' 'create' $p $_ }
}

# @fn __CopyDir
function __CopyDir([string]$from, [string]$to) {
    [void][IO.Directory]::CreateDirectory($to)
    foreach ($f in [IO.Directory]::GetFiles($from)) { [IO.File]::Copy($f, [IO.Path]::Combine($to, [IO.Path]::GetFileName($f)), $true) }
    foreach ($d in [IO.Directory]::GetDirectories($from)) { __CopyDir $d ([IO.Path]::Combine($to, [IO.Path]::GetFileName($d))) }
}

# @fn __FsCopy
# @needs __Full __FsErr __DryW __CopyDir __Throw
# Like `cp -R`: if the destination is an existing folder, copies into it.
function __FsCopy([string]$from, [string]$to) {
    if (__DryW ('fs.copy ' + $from + ' ' + $to) $to) { return }
    $a = __Full $from; $b = __Full $to
    try {
        if ([IO.Directory]::Exists($b)) { $b = [IO.Path]::Combine($b, [IO.Path]::GetFileName($a.TrimEnd([char[]]@('/', '\')))) }
        if ([IO.Directory]::Exists($a)) { __CopyDir $a $b }
        elseif ([IO.File]::Exists($a)) { [IO.File]::Copy($a, $b, $true) }
        else { __Throw ("fs.copy: '" + $from + "' does not exist") 1 }
    } catch {
        if ($_.Exception.Data.Contains('nalu_code')) { throw }
        __Throw ("fs.copy: could not copy '" + $from + "' to '" + $to + "'") 1
    }
}

# @fn __FsMove
# @needs __Full __FsErr __DryW __CopyDir __Throw
function __FsMove([string]$from, [string]$to) {
    # with --dry-run, the source counts too: moving a user's folder into a temporary one is a real change
    if (__DryW ('fs.move ' + $from + ' ' + $to) $from $to) { return }
    $a = __Full $from; $b = __Full $to
    try {
        if ([IO.Directory]::Exists($b)) { $b = [IO.Path]::Combine($b, [IO.Path]::GetFileName($a.TrimEnd([char[]]@('/', '\')))) }
        if ([IO.Directory]::Exists($a)) {
            # Directory.Move cannot cross volumes (C: to D:): then copy and remove the original, like `mv`
            if ([IO.Path]::GetPathRoot($a) -ieq [IO.Path]::GetPathRoot($b)) { [IO.Directory]::Move($a, $b) }
            else { __CopyDir $a $b; [IO.Directory]::Delete($a, $true) }
        }
        elseif ([IO.File]::Exists($a)) {
            if ([IO.File]::Exists($b)) { [IO.File]::Delete($b) }
            [IO.File]::Move($a, $b)
        }
        else { __Throw ("fs.move: '" + $from + "' does not exist") 1 }
    } catch {
        if ($_.Exception.Data.Contains('nalu_code')) { throw }
        __Throw ("fs.move: could not move '" + $from + "' to '" + $to + "'") 1
    }
}

# @fn __FsLs
# @needs __P __FsErr __Throw
# Entry names (without . and ..), in ordinal order.
function __FsLs([string]$p) {
    if (-not [IO.Directory]::Exists((__P $p))) { __Throw ("fs.ls: '" + $p + "' is not a folder") 1 }
    try { $e = [IO.Directory]::GetFileSystemEntries((__P $p)) } catch { __FsErr 'fs.ls' 'list' $p $_ }
    $names = New-Object 'System.Collections.Generic.List[string]'
    foreach ($x in $e) { $names.Add([IO.Path]::GetFileName($x)) }
    $a = $names.ToArray()
    [Array]::Sort($a, [StringComparer]::Ordinal)
    $r = [System.Collections.Generic.List[object]]::new()
    foreach ($x in $a) { $r.Add($x) }
    return ,$r
}

# @fn __FsWalk
# @needs __P __FsErr __Throw
# Files, recursive; each path is `dir` as given + `/` + the relative path with `/`; sorted.
function __FsWalk([string]$p) {
    $base = [IO.Path]::GetFullPath([IO.Path]::Combine((Get-Location).ProviderPath, (__P $p))).TrimEnd([char[]]@('/', '\'))
    if ($base.Length -eq 0) { $base = [IO.Path]::GetPathRoot((Get-Location).ProviderPath) }
    if (-not [IO.Directory]::Exists($base)) { __Throw ("fs.walk: '" + $p + "' is not a folder") 1 }
    try { $files = [IO.Directory]::GetFiles($base, '*', [IO.SearchOption]::AllDirectories) } catch { __FsErr 'fs.walk' 'read' $p $_ }
    $prefix = $p.TrimEnd([char[]]@('/', '\'))
    $list = New-Object 'System.Collections.Generic.List[string]'
    foreach ($f in $files) {
        $rel = $f.Substring($base.Length).TrimStart([char[]]@('/', '\')).Replace('\', '/')
        if ($prefix.Length -eq 0) { $list.Add($rel) } else { $list.Add($prefix + '/' + $rel) }
    }
    $a = $list.ToArray()
    [Array]::Sort($a, [StringComparer]::Ordinal)
    $r = [System.Collections.Generic.List[object]]::new()
    foreach ($x in $a) { $r.Add($x) }
    return ,$r
}

# @fn __FsGlob
# @needs __P
# * and ? only in the last component; no dotfiles unless the pattern starts with `.`; never lists the contents
# of a folder; paths keep the pattern's prefix; sorted; empty if nothing matches.
function __FsGlob([string]$pat) {
    $r = [System.Collections.Generic.List[object]]::new()
    $i = $pat.LastIndexOfAny([char[]]@('/', '\'))
    $dirPart = ''
    $last = $pat
    if ($i -ge 0) { $dirPart = $pat.Substring(0, $i + 1); $last = $pat.Substring($i + 1) }
    if ($last.IndexOfAny([char[]]@('*', '?')) -lt 0) {
        $f = __P $pat
        if ($f.Length -gt 0 -and ([IO.File]::Exists($f) -or [IO.Directory]::Exists($f))) { $r.Add($pat) }
        return ,$r
    }
    $dir = __P $dirPart
    if ($dir.Length -eq 0) { $dir = '.' }
    if (-not [IO.Directory]::Exists($dir)) { return ,$r }
    $rx = '^' + [regex]::Escape($last).Replace('\*', '.*').Replace('\?', '.') + '$'
    $names = New-Object 'System.Collections.Generic.List[string]'
    foreach ($e in [IO.Directory]::GetFileSystemEntries($dir)) {
        $n = [IO.Path]::GetFileName($e)
        if ($n.StartsWith('.') -and -not $last.StartsWith('.')) { continue }
        if ([regex]::IsMatch($n, $rx, [System.Text.RegularExpressions.RegexOptions]::CultureInvariant)) { $names.Add($n) }
    }
    $a = $names.ToArray()
    [Array]::Sort($a, [StringComparer]::Ordinal)
    foreach ($n in $a) { $r.Add($dirPart + $n) }
    return ,$r
}

# @fn __FsTemp
# @needs __Temps __FsErr __TempBase __Throw
function __FsTemp([bool]$dir) {
    $base = __TempBase
    $name = 'nalu.' + [Guid]::NewGuid().ToString('N').Substring(0, 10)
    $p = [IO.Path]::Combine($base, $name)
    try {
        if ($dir) { [void][IO.Directory]::CreateDirectory($p) } else { [IO.File]::WriteAllBytes($p, [byte[]]@()) }
    } catch {
        if ($dir) { __Throw 'fs.temp_dir: could not create a temporary folder' 1 }
        __Throw 'fs.temp_file: could not create a temporary file' 1
    }
    $script:__temps.Add([IO.Path]::GetFullPath($p))
    return $p
}

# @fn __FsSize
# @needs __P __Throw
function __FsSize([string]$p) {
    $f = __P $p
    if ([IO.File]::Exists($f)) { return [long](New-Object IO.FileInfo $f).Length }
    if ([IO.Directory]::Exists($f)) { return 0L }
    __Throw ("fs.size: '" + $p + "' does not exist") 1
}

# @fn __FsModified
# @needs __P __Epoch __Throw
function __FsModified([string]$p) {
    $f = __P $p
    if (-not ([IO.File]::Exists($f) -or [IO.Directory]::Exists($f))) { __Throw ("fs.modified: '" + $p + "' does not exist") 1 }
    return [long][Math]::Floor(([IO.File]::GetLastWriteTimeUtc($f) - $script:__epoch).TotalSeconds)
}

# @fn __FsReplace
# @needs __P __FsErr __DryW __Utf8
# Replaces every occurrence (literal text); keeps the BOM if the file had one.
function __FsReplace([string]$p, [string]$from, [string]$to) {
    if (__DryW ('fs.replace ' + $p) $p) { return }
    $f = __P $p
    try { $b = [IO.File]::ReadAllBytes($f) } catch { __FsErr 'fs.replace' 'read' $p $_ }
    $off = 0
    if ($b.Length -ge 3 -and $b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF) { $off = 3 }
    $t = $script:__utf8.GetString($b, $off, $b.Length - $off)
    if ($from.Length -gt 0) { $t = $t.Replace($from, $to) }
    $out = $script:__utf8.GetBytes($t)
    if ($off -eq 3) { $out = [byte[]](0xEF, 0xBB, 0xBF) + $out }
    try { [IO.File]::WriteAllBytes($f, [byte[]]$out) } catch { __FsErr 'fs.replace' 'write' $p $_ }
}
