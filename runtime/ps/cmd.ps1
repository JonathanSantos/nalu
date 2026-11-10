# Nalu's PowerShell runtime: external commands.
#
# Every external program runs through System.Diagnostics.Process, not `& prog`. This way the generator
# sidesteps the Windows PowerShell 5.1 pitfalls in one go:
# - arguments: the command line is built here with MSVCRT escaping (`\"`, and `\` doubled before `"`),
#   bypassing PowerShell's algorithm, which loses embedded quotes (PowerShell#1995); empty `""` works;
# - stdout/stderr inherited when there is no redirection: bytes intact, no NativeCommandError;
# - `>`/`>>`/`2>`/`2>>`/`<` copy bytes (no UTF-16 or BOM from `Out-File`);
# - `value | $ cmd` writes the exact bytes (UTF-8, no extra `\n`);
# - pipes `a | b` connect the processes byte for byte; the code is the rightmost failed stage's (pipefail).
# `echo` is translated into a one-line write (args joined by spaces), as in Bash.
# `.cmd`/`.bat` programs on Windows run via `cmd.exe /d /s /c` (`%VAR%` in the arguments is expanded by cmd).

# @fn __QArg
# Quotes an argument for the Windows command line (MSVCRT / CommandLineToArgvW rules).
function __QArg([string]$a) {
    if ($a.Length -gt 0 -and $a -notmatch '[\s"]') { return $a }
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.Append([char]'"')
    $bs = 0
    foreach ($ch in $a.ToCharArray()) {
        if ($ch -eq [char]'\') { $bs++; continue }
        if ($ch -eq [char]'"') {
            [void]$sb.Append([char]'\', 2 * $bs + 1)
            [void]$sb.Append([char]'"')
        } else {
            if ($bs -gt 0) { [void]$sb.Append([char]'\', $bs) }
            [void]$sb.Append($ch)
        }
        $bs = 0
    }
    if ($bs -gt 0) { [void]$sb.Append([char]'\', 2 * $bs) }
    [void]$sb.Append([char]'"')
    return $sb.ToString()
}

# @fn __CmdLine
# @needs __QArg
function __CmdLine($words, [int]$from) {
    $parts = New-Object 'System.Collections.Generic.List[string]'
    for ($i = $from; $i -lt $words.Count; $i++) { $parts.Add((__QArg ([string]$words[$i]))) }
    return [string]::Join(' ', $parts)
}

# @fn __StartProc
# @needs __CmdLine __QArg __IsWin
function __StartProc([string]$exe, $words, [bool]$rin, [bool]$rout, [bool]$rerr) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $args1 = __CmdLine $words 1
    if ($script:__isWin -and ($exe.EndsWith('.cmd', [StringComparison]::OrdinalIgnoreCase) -or $exe.EndsWith('.bat', [StringComparison]::OrdinalIgnoreCase))) {
        $psi.FileName = $env:ComSpec
        $psi.Arguments = '/d /s /c "' + (__QArg $exe) + ' ' + $args1 + '"'
    } else {
        $psi.FileName = $exe
        $psi.Arguments = $args1
    }
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = (Get-Location).ProviderPath
    $psi.RedirectStandardInput = $rin
    $psi.RedirectStandardOutput = $rout
    $psi.RedirectStandardError = $rerr
    return [System.Diagnostics.Process]::Start($psi)
}

# @fn __WaitTask
function __WaitTask($t) {
    if ($null -eq $t) { return }
    try { $t.Wait() } catch { }
}

# @fn __Exec
# @needs __Which __StartProc __WaitTask __P __Utf8 __ParFlag
# Runs a command. $spec: s = stages (lists of words), o/oa = stdout file (append),
# e/ea = stderr file, i = input file, d/hd = data for the input, e2o = 2>&1.
# Inside a task of `parallel`, the first stage reads an empty stdin (like a background job in Bash).
# Returns @{ code; prog; out } (out only with $capture).
function __Exec($spec, [bool]$capture) {
    $stages = $spec.s
    $n = $stages.Count
    $exes = New-Object 'System.Collections.Generic.List[object]'
    for ($k = 0; $k -lt $n; $k++) {
        $w = $stages[$k]
        $name = [string]$w[0]
        if ($name -eq 'echo') { $exes.Add('echo'); continue }
        $p = __Which $name
        if ($null -eq $p) { return @{ code = 127; prog = $name; out = ''; notfound = $true } }
        $exes.Add($p)
    }
    $outFile = $null; $errFile = $null; $inFile = $null; $mem = $null; $console = $null
    $last = $n - 1
    $procs = New-Object 'System.Collections.Generic.List[object]'
    $feeds = New-Object 'System.Collections.Generic.List[object]'
    $sinks = New-Object 'System.Collections.Generic.List[object]'
    $pendingEcho = $null
    try {
        if ($null -ne $spec.o) {
            $mode = [IO.FileMode]::Create
            if ($spec.oa) { $mode = [IO.FileMode]::Append }
            $outFile = New-Object IO.FileStream ((__P ([string]$spec.o))), $mode, ([IO.FileAccess]::Write)
        }
        if ($null -ne $spec.e) {
            $mode = [IO.FileMode]::Create
            if ($spec.ea) { $mode = [IO.FileMode]::Append }
            $errFile = New-Object IO.FileStream ((__P ([string]$spec.e))), $mode, ([IO.FileAccess]::Write)
        }
        if ($null -ne $spec.i) { $inFile = [IO.File]::OpenRead((__P ([string]$spec.i))) }
        if ($capture) { $mem = New-Object IO.MemoryStream }
        # Destination of the last stage's stdout (null = inherited).
        $dest = $outFile
        if ($capture) { $dest = $mem }
        if ($null -eq $dest -and $spec.e2o) { $console = [Console]::OpenStandardOutput(); $dest = $console }
        $prevOut = $null
        $codes = New-Object 'System.Collections.Generic.List[int]'
        for ($k = 0; $k -lt $n; $k++) {
            $w = $stages[$k]
            $isLast = ($k -eq $last)
            $data = $null
            if ($k -eq 0 -and $spec.hd) { $data = $script:__utf8.GetBytes([string]$spec.d) }
            if ($exes[$k] -eq 'echo') {
                # `echo -n` as in Bash: no trailing newline.
                $parts = New-Object 'System.Collections.Generic.List[string]'
                $nl = "`n"
                $j = 1
                if ($w.Count -gt 1 -and [string]$w[1] -ceq '-n') { $nl = ''; $j = 2 }
                for (; $j -lt $w.Count; $j++) { $parts.Add([string]$w[$j]) }
                $bytes = $script:__utf8.GetBytes([string]::Join(' ', $parts) + $nl)
                $prevOut = $null
                if ($isLast) {
                    if ($null -ne $dest) { $dest.Write($bytes, 0, $bytes.Length) }
                    else { [Console]::Out.Flush(); $so = [Console]::OpenStandardOutput(); $so.Write($bytes, 0, $bytes.Length); $so.Flush() }
                } else {
                    $pendingEcho = $bytes
                }
                $procs.Add($null)
                continue
            }
            $rin = ($k -gt 0) -or ($null -ne $data) -or ($null -ne $inFile) -or $script:__parTask
            $rout = (-not $isLast) -or ($null -ne $dest)
            $rerr = $isLast -and (($null -ne $errFile) -or ($spec.e2o -and $null -ne $dest))
            $p = __StartProc ([string]$exes[$k]) $w $rin $rout $rerr
            $procs.Add($p)
            if ($rin) {
                $sin = $p.StandardInput.BaseStream
                if ($null -ne $prevOut) {
                    $feeds.Add(@($prevOut.CopyToAsync($sin), $sin))
                } elseif ($null -ne $pendingEcho) {
                    try { $sin.Write($pendingEcho, 0, $pendingEcho.Length) } catch { }
                    try { $sin.Close() } catch { }
                    $pendingEcho = $null
                } elseif ($null -ne $data) {
                    $feeds.Add(@($null, $sin, $data))
                } elseif ($null -ne $inFile) {
                    $feeds.Add(@($inFile.CopyToAsync($sin), $sin))
                } else {
                    try { $sin.Close() } catch { }
                }
            }
            $prevOut = $null
            if (-not $isLast) { $prevOut = $p.StandardOutput.BaseStream }
            elseif ($rout) { $sinks.Add($p.StandardOutput.BaseStream.CopyToAsync($dest)) }
            if ($rerr) {
                $ed = $errFile
                if ($null -eq $ed) { $ed = $dest }
                $sinks.Add($p.StandardError.BaseStream.CopyToAsync($ed))
            }
        }
        # Input data: written once every stage is already draining its output.
        foreach ($f in $feeds) {
            if ($null -eq $f[0]) {
                try { $f[1].Write($f[2], 0, $f[2].Length) } catch { }
                try { $f[1].Close() } catch { }
            }
        }
        foreach ($f in $feeds) {
            if ($null -ne $f[0]) {
                __WaitTask $f[0]
                try { $f[1].Close() } catch { }
            }
        }
        foreach ($t in $sinks) { __WaitTask $t }
        $code = 0
        $prog = ''
        for ($k = 0; $k -lt $n; $k++) {
            $p = $procs[$k]
            if ($null -eq $p) { continue }
            $p.WaitForExit()
            if ($p.ExitCode -ne 0) { $code = $p.ExitCode; $prog = [string]$stages[$k][0] }
        }
        $out = ''
        if ($capture) {
            $b = $mem.ToArray()
            $off = 0
            if ($b.Length -ge 3 -and $b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF) { $off = 3 }
            $out = $script:__utf8.GetString($b, $off, $b.Length - $off)
            $out = $out.TrimEnd([char[]]@("`n", "`r"))
        }
        return @{ code = $code; prog = $prog; out = $out; notfound = $false }
    } finally {
        foreach ($s in @($outFile, $errFile, $inFile, $console)) { if ($null -ne $s) { try { $s.Dispose() } catch { } } }
        foreach ($p in $procs) { if ($null -ne $p) { try { $p.Dispose() } catch { } } }
    }
}

# @fn __CmdFail
# @needs __Throw
function __CmdFail($r) {
    if ($r.notfound) { __Throw ('command `' + $r.prog + '` was not found') 127 }
    __Throw ('command `' + $r.prog + '` failed with exit code ' + $r.code) $r.code
}

# @fn __Cmd
# @needs __Exec __CmdFail __ErrLine
# `$ cmd` as a statement: shows it in --verbose, only shows it in --dry-run, fails if the code is not 0.
function __Cmd($spec) {
    if ($script:__dry) { __ErrLine ('[dry-run] ' + $spec.t); return }
    if ($script:__verbose) { __ErrLine ('+ ' + $spec.t) }
    [Console]::Out.Flush()
    $r = __Exec $spec $false
    if ($r.code -ne 0) { __CmdFail $r }
}

# @fn __Capture
# @needs __Exec __CmdFail __ErrLine
# `$(cmd)`: stdout without the trailing newlines; also runs in --dry-run.
function __Capture($spec) {
    if ($script:__verbose) { __ErrLine ('+ ' + $spec.t) }
    $r = __Exec $spec $true
    if ($r.code -ne 0) { __CmdFail $r }
    return [string]$r.out
}

# @fn __CmdOk
# @needs __Exec __ErrLine
# `if $ cmd {`: true if the code is 0; never fails; also runs in --dry-run.
function __CmdOk($spec) {
    if ($script:__verbose) { __ErrLine ('+ ' + $spec.t) }
    [Console]::Out.Flush()
    $r = __Exec $spec $false
    return ($r.code -eq 0)
}
