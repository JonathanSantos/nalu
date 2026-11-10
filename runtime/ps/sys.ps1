# Nalu's PowerShell runtime: env, sys, proc, time, math, log, ui, http.

# @fn __EnvGet
# @needs __Throw
function __EnvGet([string]$n) {
    $v = [Environment]::GetEnvironmentVariable($n)
    if ($null -eq $v) { __Throw ("environment variable " + $n + " is not set") 1 }
    return $v
}

# @fn __SysArch
function __SysArch {
    $a = ''
    try { $a = [string][System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture } catch { $a = [string]$env:PROCESSOR_ARCHITECTURE }
    switch -regex ($a.ToLowerInvariant()) {
        '^(x64|amd64)$' { return 'x64' }
        '^(arm64|aarch64)$' { return 'arm64' }
        '^(x86|i386|i686)$' { return 'x86' }
        default { return $a.ToLowerInvariant() }
    }
}

# @fn __SysIsAdmin
# @needs __IsWin
function __SysIsAdmin {
    if ($script:__isWin) {
        $id = [Security.Principal.WindowsIdentity]::GetCurrent()
        return (New-Object Security.Principal.WindowsPrincipal $id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    }
    return [Environment]::UserName -eq 'root'
}

# @fn __SysCd
# @needs __P __Throw
# Changes PowerShell's folder **and** the process's (.NET and external programs use the process's).
function __SysCd([string]$d) {
    $p = __P $d
    if ($p.Length -eq 0 -or -not [IO.Directory]::Exists($p)) { __Throw ("sys.cd: could not enter '" + $d + "'") 1 }
    Set-Location -LiteralPath $p
    [Environment]::CurrentDirectory = (Get-Location).ProviderPath
}

# @fn __Which
# Path of an external program (applications only, not cmdlets/aliases), cached.
$script:__which = @{}
function __Which([string]$name) {
    if ($name.Length -eq 0) { return $null }
    if ($script:__which.ContainsKey($name)) { return $script:__which[$name] }
    $p = $null
    if ($name.IndexOfAny([char[]]@('/', '\')) -ge 0) {
        $f = [IO.Path]::GetFullPath([IO.Path]::Combine((Get-Location).ProviderPath, $name))
        if ([IO.File]::Exists($f)) { $p = $f }
    } else {
        $c = Get-Command -Name $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($null -ne $c) { $p = $c.Path }
    }
    $script:__which[$name] = $p
    return $p
}

# @fn __ProcWhich
# @needs __Which __Throw
function __ProcWhich([string]$n) {
    $p = __Which $n
    if ($null -eq $p) { __Throw ("'" + $n + "' was not found in PATH") 127 }
    return $p
}

# @fn __Deps
# @needs __Which __ErrLine
function __Deps($names) {
    if ($script:__dry) { return }
    $miss = New-Object 'System.Collections.Generic.List[string]'
    foreach ($n in $names) { if ($null -eq (__Which $n)) { $miss.Add($n) } }
    if ($miss.Count -gt 0) {
        __ErrLine ('error: this script needs: ' + [string]::Join(', ', $miss))
        exit 127
    }
}

# @fn __Epoch
# No DateTimeOffset.FromUnixTimeSeconds/ToUnixTimeSeconds: they are .NET 4.6, and WMF 5.1 runs on 4.5.2.
$script:__epoch = New-Object DateTime 1970, 1, 1, 0, 0, 0, ([DateTimeKind]::Utc)

# @fn __Now
# @needs __Epoch
function __Now { return [long][Math]::Floor(([DateTime]::UtcNow - $script:__epoch).TotalSeconds) }

# @fn __NowMs
# @needs __Epoch
function __NowMs { return [long][Math]::Floor(([DateTime]::UtcNow - $script:__epoch).TotalMilliseconds) }

# @fn __TimeAt
# @needs __Epoch
# Unix instant (seconds) -> local DateTime; -1 = now.
function __TimeAt($t) {
    $t = [long]$t
    if ($t -lt 0) { return [DateTime]::Now }
    return $script:__epoch.AddSeconds($t).ToLocalTime()
}

# @fn __TimeFormat
# @needs __TimeAt __Throw __Epoch
# Only %Y %y %m %d %H %M %S %j %s %%; any other % fails.
function __TimeFormat([string]$fmt, $t) {
    $d = __TimeAt $t
    $inv = [System.Globalization.CultureInfo]::InvariantCulture
    $sb = New-Object System.Text.StringBuilder
    for ($i = 0; $i -lt $fmt.Length; $i++) {
        $c = $fmt[$i]
        if ($c -ne [char]'%') { [void]$sb.Append($c); continue }
        $i++
        if ($i -ge $fmt.Length) { __Throw 'time.format: a lone % at the end of the format' 1 }
        switch -casesensitive ([string]$fmt[$i]) {
            'Y' { [void]$sb.Append($d.ToString('yyyy', $inv)) }
            'y' { [void]$sb.Append($d.ToString('yy', $inv)) }
            'm' { [void]$sb.Append($d.ToString('MM', $inv)) }
            'd' { [void]$sb.Append($d.ToString('dd', $inv)) }
            'H' { [void]$sb.Append($d.ToString('HH', $inv)) }
            'M' { [void]$sb.Append($d.ToString('mm', $inv)) }
            'S' { [void]$sb.Append($d.ToString('ss', $inv)) }
            'j' { [void]$sb.Append(([string]$d.DayOfYear).PadLeft(3, [char]'0')) }
            's' { [void]$sb.Append([string][long][Math]::Floor(($d.ToUniversalTime() - $script:__epoch).TotalSeconds)) }
            '%' { [void]$sb.Append('%') }
            default { __Throw ('time.format: %' + $fmt[$i] + ' is not supported (use %Y %y %m %d %H %M %S %j %s %%)') 1 }
        }
    }
    return $sb.ToString()
}

# @fn __Sleep
function __Sleep($ms) { if ([long]$ms -gt 0) { Start-Sleep -Milliseconds ([int][long]$ms) } }

# @fn __Log
# @needs __ErrLine
function __Log([string]$prefix, [string]$text) { __ErrLine ($prefix + $text) }

# @fn __HasTty
# @needs __ParFlag
# A task of `parallel` never has a terminal (in Bash it is a background job, with stdin from /dev/null).
function __HasTty {
    if ($script:__parTask) { return $false }
    try { return -not [Console]::IsInputRedirected } catch { return $false }
}

# @fn __UiConfirm
# @needs __HasTty
function __UiConfirm([string]$q) {
    if (-not (__HasTty)) { return $false }
    [Console]::Error.Write($q + ' [y/N] ')
    $a = [Console]::In.ReadLine()
    if ($null -eq $a) { return $false }
    return @('s', 'sim', 'y', 'yes') -contains $a.Trim().ToLowerInvariant()
}

# @fn __UiAsk
# @needs __HasTty
function __UiAsk([string]$q, [string]$fallback) {
    if (-not (__HasTty)) { return $fallback }
    [Console]::Error.Write($q + ' ')
    $a = [Console]::In.ReadLine()
    if ($null -eq $a -or $a.Length -eq 0) { return $fallback }
    return $a
}

# @fn __UiPassword
# @needs __HasTty __Throw
function __UiPassword([string]$q) {
    if (-not (__HasTty)) { __Throw ('ui.password: no terminal to ask: ' + $q) 1 }
    [Console]::Error.Write($q + ' ')
    $sb = New-Object System.Text.StringBuilder
    while ($true) {
        $k = [Console]::ReadKey($true)
        if ($k.Key -eq [ConsoleKey]::Enter) { break }
        if ($k.Key -eq [ConsoleKey]::Backspace) { if ($sb.Length -gt 0) { [void]$sb.Remove($sb.Length - 1, 1) }; continue }
        [void]$sb.Append($k.KeyChar)
    }
    [Console]::Error.Write("`n")
    return $sb.ToString()
}

# @fn __UiChoose
# @needs __HasTty __Throw
function __UiChoose([string]$q, $opts) {
    if ($opts.Count -eq 0) { __Throw 'ui.choose: no options' 1 }
    if (-not (__HasTty)) { __Throw ('ui.choose: no terminal to ask: ' + $q) 1 }
    [Console]::Error.Write($q + "`n")
    for ($i = 0; $i -lt $opts.Count; $i++) { [Console]::Error.Write(('  ' + ($i + 1) + ') ' + $opts[$i]) + "`n") }
    [Console]::Error.Write('> ')
    $a = [Console]::In.ReadLine()
    $n = 0
    if ($null -ne $a -and [int]::TryParse($a.Trim(), [ref]$n) -and $n -ge 1 -and $n -le $opts.Count) { return $opts[$n - 1] }
    __Throw ('ui.choose: invalid choice: ' + $a) 1
}

# @fn __Http
# @needs __Throw __Full
# Invoke-WebRequest with TLS 1.2 (5.1 ships without it), no progress bar (it makes 5.1 slow).
# Headers "Name: value"; the token becomes Authorization: Bearer in memory only. In 5.1, User-Agent and
# Content-Type cannot go in -Headers: they go in their own parameters.
function __Http([string]$url, $timeoutMs, [string]$mode, [string]$to, $headers, [string]$bearer, [string]$body, [string]$ct, [string]$method = '') {
    $ProgressPreference = 'SilentlyContinue'
    try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }
    if ($mode -eq 'request') {
        if (@('POST', 'PUT', 'PATCH', 'DELETE') -cnotcontains $method) { __Throw ("http.request: '" + $method + "' is not a method it sends (POST, PUT, PATCH or DELETE)") 1 }
        if ($script:__dry) { [Console]::Error.WriteLine('[dry-run] http.request ' + $method + ' ' + $url); return '' }
    }
    if ($mode -eq 'post' -and $script:__dry) { [Console]::Error.WriteLine('[dry-run] http.post ' + $url); return '' }
    $sec = [int][Math]::Max(1, [Math]::Ceiling([double][long]$timeoutMs / 1000))
    $h = @{}
    $ua = $null
    if ($null -ne $headers) {
        foreach ($x in $headers) {
            $x = [string]$x
            $i = $x.IndexOf(':')
            if ($i -lt 1) { continue }
            $name = $x.Substring(0, $i).Trim(); $val = $x.Substring($i + 1).Trim()
            if ($name -eq 'User-Agent') { $ua = $val }
            elseif ($name -eq 'Content-Type') { $ct = $val }
            else { $h[$name] = $val }
        }
    }
    if ($bearer.Length -gt 0) { $h['Authorization'] = 'Bearer ' + $bearer }
    $p = @{ Uri = $url; UseBasicParsing = $true; TimeoutSec = $sec; Headers = $h; ErrorAction = 'Stop' }
    if ($null -ne $ua) { $p['UserAgent'] = $ua }
    # PowerShell 6+ validates the header format (a token with quotes would fail); curl and 5.1 send it as is
    if ($PSVersionTable.PSVersion.Major -ge 6) { $p['SkipHeaderValidation'] = $true }
    if ($mode -eq 'post') {
        $p['Method'] = 'Post'
        $p['ContentType'] = $ct
        $p['Body'] = (New-Object System.Text.UTF8Encoding $false).GetBytes($body)
    }
    if ($mode -eq 'request') {
        $p['Method'] = $method
        # an empty body sends none
        if ($body.Length -gt 0) {
            $p['ContentType'] = $ct
            $p['Body'] = (New-Object System.Text.UTF8Encoding $false).GetBytes($body)
        }
    }
    $status = 0
    $resp = $null
    $part = $null
    try {
        if ($mode -eq 'download') {
            # download alongside, swap in only at the end: a failed download leaves no half-written file
            $dest = __Full $to
            $part = $dest + '.nalu-partial'
            $p['OutFile'] = $part
            Invoke-WebRequest @p | Out-Null
            Move-Item -LiteralPath $part -Destination $dest -Force
            return
        }
        $resp = Invoke-WebRequest @p
        $status = [int]$resp.StatusCode
    } catch {
        if ($null -ne $part -and [IO.File]::Exists($part)) { Remove-Item -LiteralPath $part -Force -ErrorAction SilentlyContinue }
        $ex = $_.Exception
        $r = $null
        if ($null -ne $ex.PSObject.Properties['Response']) { $r = $ex.Response }
        if ($null -ne $r) { $status = [int]$r.StatusCode }
        else { __Throw ('http.' + $mode + ': could not reach ' + $url) 1 }
        if ($mode -eq 'status') { return [long]$status }
        __Throw ('http.' + $mode + ': ' + $url + ' answered ' + $status) 1
    }
    if ($mode -eq 'status') { return [long]$status }
    $c = $resp.Content
    if ($c -is [byte[]]) { return (New-Object System.Text.UTF8Encoding $false).GetString($c) }
    return [string]$c
}

# @fn __ParFlag
# $true while a task of `parallel` runs: like a background job in Bash, it has no terminal (empty stdin).
$script:__parTask = $false

# @fn __ParCopy
# A task's own copy of a value: lists and maps/structs (deep), and the runtime's mutable state (the
# cache of program paths, the list of temporaries). Text, numbers, dates and encodings are immutable.
function __ParCopy($v) {
    if ($v -is [System.Collections.Generic.List[object]]) {
        $n = [System.Collections.Generic.List[object]]::new($v)
        # lists are homogeneous: only lists of lists or of structs need more than this copy
        if ($n.Count -gt 0 -and ($n[0] -is [System.Collections.Generic.List[object]] -or $n[0] -is [System.Collections.Specialized.OrderedDictionary])) {
            for ($i = 0; $i -lt $n.Count; $i++) { $n[$i] = __ParCopy $n[$i] }
        }
        return ,$n
    }
    if ($v -is [System.Collections.Specialized.OrderedDictionary]) {
        $n = [System.Collections.Specialized.OrderedDictionary]::new([StringComparer]::Ordinal)
        foreach ($k in $v.Keys) {
            $x = $v[$k]
            if ($x -is [System.Collections.Generic.List[object]] -or $x -is [System.Collections.Specialized.OrderedDictionary]) { $x = __ParCopy $x }
            $n[$k] = $x
        }
        return ,$n
    }
    if ($v -is [hashtable]) { return ,$v.Clone() }
    if ($v -is [System.Collections.Generic.List[string]]) { return ,([System.Collections.Generic.List[string]]::new($v)) }
    return ,$v
}

# @fn __ParEnv
# Puts the process environment back as it was in $snap. As in Bash, where each task is a subshell, an
# env.set inside a task does not leak out of it (nor into the next task, when they run one at a time).
function __ParEnv($snap) {
    $now = [Environment]::GetEnvironmentVariables()
    foreach ($k in @($now.Keys)) {
        if (-not $snap.Contains($k)) { try { [Environment]::SetEnvironmentVariable([string]$k, $null) } catch { } }
    }
    foreach ($k in @($snap.Keys)) {
        if (-not $now.Contains($k) -or -not [string]::Equals([string]$now[$k], [string]$snap[$k])) {
            try { [Environment]::SetEnvironmentVariable([string]$k, [string]$snap[$k]) } catch { }
        }
    }
}

# @fn __ParOut
# What a task writes to the PowerShell pipeline (only `native` code does; Write-Host arrives as stream 6)
# goes to stdout, as it would in the script itself.
function __ParOut {
    process {
        if ($_ -is [System.Management.Automation.InformationRecord]) { [Console]::Out.Write([string]$_.MessageData + "`n") }
        else { [Console]::Out.Write(($_ | Out-String)) }
    }
}

# @fn __ParTask
# @needs __ErrOf __ParCopy __ParOut __ParEnv __ParFlag
# Task $i of a `parallel`: calls the function $fn (with a copy of the item $it when $has). With $glob, it
# runs in a runspace of the pool and first sets up the script's state: a copy of the script's variables
# ($glob, as globals: in a runspace `$script:` is the global scope), the enclosing function's variables
# ($loc, only read: Nalu forbids tasks to change outside variables) and the folder; with $envs, also the
# environment. Writes @{ ok; ret } or @{ ok; msg; code } to $slots[$i]. An exit() inside the task leaves
# the slot empty, as in Bash, where the task is a subshell ("task N finished without a result").
function __ParTask([string]$fn, [bool]$has, $it, $glob, $loc, [string]$cwd, $envs, $slots, [int]$i) {
    if ($null -ne $glob) {
        Set-StrictMode -Version 2
        $ErrorActionPreference = 'Stop'
        $ProgressPreference = 'SilentlyContinue'
    }
    try {
        if ($null -ne $envs) { __ParEnv $envs }
        if ($null -ne $glob) {
            foreach ($k in @($glob.Keys)) { $ExecutionContext.SessionState.PSVariable.Set('global:' + $k, (__ParCopy $glob[$k])) }
            $script:__ret = $null
            $script:__parTask = $true
            Set-Location -LiteralPath $cwd
            [Environment]::CurrentDirectory = $cwd
        }
        $x = $null
        if ($has) { $x = __ParCopy $it }
        & {
            if ($null -ne $loc) { foreach ($k in @($loc.Keys)) { $ExecutionContext.SessionState.PSVariable.Set($k, $loc[$k]) } }
            if ($has) { & $fn $x 6>&1 | __ParOut } else { & $fn 6>&1 | __ParOut }
        }
        $slots[$i] = @{ ok = $true; ret = $script:__ret }
    } catch {
        $e = __ErrOf $_
        $slots[$i] = @{ ok = $false; msg = $e['message']; code = $e['code'] }
    }
}

# @fn __ParRun
# @needs __ParTask __ParEnv __ParFlag __ErrOf __Map
# `parallel`: runs the tasks in a RunspacePool (Windows PowerShell 5.1 has no ForEach-Object -Parallel), at
# most $max at a time, and waits for all of them. $fns: the task functions (one per task; for `parallel for`
# a single one, called with each item of $items). $names: the variables of the enclosing function that the
# tasks may read. $serial: the tasks change the folder or the environment, which belong to the whole
# process, so they run one at a time. As in Bash: results in the order of the list; every task runs even if
# others fail; the errors are joined in that order ("parallel: a; b") with the code of the first one; output
# is not buffered; a task's sys.cd and env.set do not leak out. Without a RunspacePool (Constrained Language,
# for instance) the tasks run one after another, here. Returns @{ ok; res; err }.
function __ParRun($fns, $items, $max, $names, [bool]$serial) {
    $arr = $null
    $n = $fns.Count
    if ($null -ne $items) { $arr = $items.ToArray(); $n = $arr.Length }
    $max = [long]$max
    if ($serial -or $max -lt 1) { $max = 1L }
    if ($max -gt $n) { $max = [long]$n }
    $slots = New-Object 'object[]' $n
    $cwd = (Get-Location).ProviderPath
    $env0 = [Environment]::GetEnvironmentVariables()
    $envs = $null
    if ($serial) { $envs = $env0 }
    if ($n -gt 0) {
        $pool = $null
        $glob = @{}
        $loc = @{}
        try {
            # the state the tasks see: the script's variables (o_*: Nalu's; __*: the runtime's, except the
            # top-level defers and the JSON cache) and the enclosing function's
            foreach ($v in @(Get-Variable -Scope Script)) {
                $k = $v.Name
                if ($k -like 'o_*' -or ($k -like '__*' -and $k -notlike '__js*' -and $k -ne '__defers' -and $k -ne '__code' -and $k -ne '__ret')) { $glob[$k] = $v.Value }
            }
            foreach ($k in $names) {
                $v = Get-Variable -Name $k -ErrorAction SilentlyContinue
                if ($null -ne $v) { $loc[$k] = $v.Value }
            }
            # the script's functions (runtime, user's, tasks, lambdas, defers) in every runspace
            $iss = [System.Management.Automation.Runspaces.InitialSessionState]::CreateDefault2()
            foreach ($f in @(Get-ChildItem -Path function:)) {
                if ($f.Name -like '__*' -or $f.Name -like 'fn_*') {
                    $iss.Commands.Add((New-Object System.Management.Automation.Runspaces.SessionStateFunctionEntry -ArgumentList $f.Name, $f.Definition))
                }
            }
            # no PSHost: an exit() inside a task must not end (or set the code of) the whole script
            $pool = [runspacefactory]::CreateRunspacePool($iss)
            [void]$pool.SetMaxRunspaces([int]$max)
            $pool.Open()
        } catch {
            $pool = $null
        }
        if ($null -ne $pool) {
            $jobs = New-Object 'object[]' $n
            try {
                for ($i = 0; $i -lt $n; $i++) {
                    $fn = ''; $it = $null; $has = $false
                    if ($null -eq $arr) { $fn = [string]$fns[$i] } else { $fn = [string]$fns[0]; $it = $arr[$i]; $has = $true }
                    try {
                        $ps = [powershell]::Create()
                        $ps.RunspacePool = $pool
                        [void]$ps.AddCommand('__ParTask').AddArgument($fn).AddArgument($has).AddArgument($it).AddArgument($glob).AddArgument($loc).AddArgument($cwd).AddArgument($envs).AddArgument($slots).AddArgument($i)
                        $jobs[$i] = @($ps, $ps.BeginInvoke())
                    } catch {
                        $e = __ErrOf $_
                        $slots[$i] = @{ ok = $false; msg = $e['message']; code = $e['code'] }
                    }
                }
                for ($i = 0; $i -lt $n; $i++) {
                    $j = $jobs[$i]
                    if ($null -eq $j) { continue }
                    # waits in short steps, so that Ctrl+C can stop the script
                    while (-not $j[1].AsyncWaitHandle.WaitOne(200)) { }
                    try { [void]$j[0].EndInvoke($j[1]) } catch {
                        if ($null -eq $slots[$i]) { $e = __ErrOf $_; $slots[$i] = @{ ok = $false; msg = $e['message']; code = $e['code'] } }
                    }
                }
            } finally {
                foreach ($j in $jobs) {
                    if ($null -eq $j) { continue }
                    if (-not $j[1].IsCompleted) { try { $j[0].Stop() } catch { } }
                    try { $j[0].Dispose() } catch { }
                }
                try { $pool.Close() } catch { }
                try { $pool.Dispose() } catch { }
            }
        } else {
            $flag = $script:__parTask
            $script:__parTask = $true
            try {
                for ($i = 0; $i -lt $n; $i++) {
                    $fn = ''; $it = $null; $has = $false
                    if ($null -eq $arr) { $fn = [string]$fns[$i] } else { $fn = [string]$fns[0]; $it = $arr[$i]; $has = $true }
                    __ParTask $fn $has $it $null $null '' $envs $slots $i
                }
            } finally {
                $script:__parTask = $flag
            }
        }
        __ParEnv $env0
        [Environment]::CurrentDirectory = $cwd
    }
    $res = [System.Collections.Generic.List[object]]::new()
    $msg = ''
    $code = $null
    for ($i = 0; $i -lt $n; $i++) {
        $s = $slots[$i]
        if ($null -eq $s) {
            if ($msg.Length -gt 0) { $msg += '; ' }
            $msg += 'task ' + ($i + 1) + ' finished without a result'
            [void]$res.Add($null)
        } elseif (-not $s['ok']) {
            if ($msg.Length -gt 0) { $msg += '; ' }
            $msg += [string]$s['msg']
            if ($null -eq $code) { $code = [long]$s['code'] }
            [void]$res.Add($null)
        } else {
            [void]$res.Add($s['ret'])
        }
    }
    if ($null -eq $code) { $code = 1L }
    # as in Bash (`__par_collect`): it fails when the joined message is not empty
    return @{ ok = ($msg.Length -eq 0); res = $res; err = (__Map @('message', ('parallel: ' + $msg), 'code', $code)) }
}

# ---- pkg ----------------------------------------------------------------------------------------

# @fn __PkgManager
# @needs __Which
function __PkgManager {
    foreach ($m in @('winget', 'choco', 'scoop')) { if ($null -ne (__Which $m)) { return $m } }
    return ''
}

# @fn __PkgInstall
# @needs __PkgManager __Which __StartProc __Throw
# In --dry-run only shows. The manager's output goes to stderr: stdout is the script's.
function __PkgInstall($names) {
    if ($null -eq $names -or $names.Count -eq 0) { return }
    $list = New-Object 'System.Collections.Generic.List[string]'
    foreach ($n in $names) { $list.Add([string]$n) }
    $all = [string]::Join(' ', $list)
    if ($script:__dry) { [Console]::Error.WriteLine('[dry-run] pkg.install ' + $all); return }
    $m = __PkgManager
    if ($m -eq '') { __Throw 'pkg.install: no package manager found' 1 }
    $exe = __Which $m
    $runs = New-Object 'System.Collections.Generic.List[object]'
    if ($m -eq 'winget') {
        # winget takes one package at a time
        foreach ($n in $list) { $runs.Add(@('winget', 'install', $n, '--silent', '--accept-package-agreements', '--accept-source-agreements')) }
    } elseif ($m -eq 'choco') {
        $runs.Add(@('choco', 'install', '-y') + $list.ToArray())
    } else {
        $runs.Add(@('scoop', 'install') + $list.ToArray())
    }
    foreach ($w in $runs) {
        [Console]::Out.Flush()
        $p = __StartProc $exe $w $true $true $false
        try { $p.StandardInput.Close() } catch { }
        $err = [Console]::OpenStandardError()
        try { $p.StandardOutput.BaseStream.CopyTo($err); $err.Flush() } catch { }
        $p.WaitForExit()
        $code = $p.ExitCode
        $p.Dispose()
        # winget: -1978335189 = already installed and nothing newer; that is what was asked for
        if ($code -ne 0 -and -not ($m -eq 'winget' -and $code -eq -1978335189)) {
            __Throw ('pkg.install: ' + $m + ' could not install: ' + $all) 1
        }
    }
}
