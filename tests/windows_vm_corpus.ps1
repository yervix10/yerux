# Runs inside the Server 2022 VM. The host prepends $Archive.
$ErrorActionPreference = 'Stop'
$root = Join-Path $env:TEMP ([Guid]::NewGuid().ToString())
[void](New-Item -ItemType Directory $root)
try {
    $zip = Join-Path $root 'corpus.zip'
    (New-Object Net.WebClient).DownloadFile($Archive, $zip)
    $dir = Join-Path $root 'corpus'
    Expand-Archive -LiteralPath $zip -DestinationPath $dir
    $cases = Get-Content (Join-Path $dir 'cases.json') -Raw | ConvertFrom-Json
    $env:YX_IO_TEST = 'hello_env'
    foreach ($case in $cases) {
        $p = New-Object Diagnostics.Process
        $stdout = New-Object IO.MemoryStream
        $stderr = New-Object IO.MemoryStream
        try {
            $p.StartInfo.FileName = Join-Path $dir ($case.name + '.exe')
            $p.StartInfo.WorkingDirectory = $dir
            $p.StartInfo.Arguments = $case.arguments
            $p.StartInfo.UseShellExecute = $false
            $p.StartInfo.CreateNoWindow = $true
            $p.StartInfo.RedirectStandardOutput = $true
            $p.StartInfo.RedirectStandardError = $true
            $p.StartInfo.RedirectStandardInput = $true
            [void]$p.Start()
            $readOut = $p.StandardOutput.BaseStream.CopyToAsync($stdout)
            $readErr = $p.StandardError.BaseStream.CopyToAsync($stderr)
            if ($case.stdin) {
                $inputBytes = [Convert]::FromBase64String($case.stdin)
                $p.StandardInput.BaseStream.Write($inputBytes, 0, $inputBytes.Length)
            }
            $p.StandardInput.BaseStream.Close()
            if (!$p.WaitForExit(15000)) {
                $p.Kill()
                $p.WaitForExit()
                "FAIL $($case.name) timeout"
                continue
            }
            [void]$readOut.GetAwaiter().GetResult()
            [void]$readErr.GetAwaiter().GetResult()
            $out = [Convert]::ToBase64String($stdout.ToArray())
            $err = [Convert]::ToBase64String($stderr.ToArray())
            if ($p.ExitCode -eq $case.code -and $out -ceq $case.stdout -and
                ($null -eq $case.stderr -or $err -ceq $case.stderr)) {
                "PASS $($case.name)"
            } else {
                "FAIL $($case.name) exit=$($p.ExitCode) expected=$($case.code) stdout=$out stderr=$err"
            }
        } catch {
            "FAIL $($case.name) $($_.Exception.Message)"
        } finally {
            $p.Dispose()
            $stdout.Dispose()
            $stderr.Dispose()
        }
    }
    "windows_vm_corpus: complete $($cases.Count)"
} finally {
    Remove-Item $root -Recurse -Force
}
