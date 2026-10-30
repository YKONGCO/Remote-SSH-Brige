param([int]$ProcessId,[string]$Executable,[string]$AlternateExecutable)
$ErrorActionPreference='Stop'
& (Join-Path $PSScriptRoot 'window-check.ps1') -ProcessId $ProcessId -CheckTray
$window=[DeskWindowCheck]::Find($ProcessId,([string][char]0x8fdc+[char]0x7a0b+[char]0x7ba1+[char]0x7406),$null)
foreach($candidate in @($Executable,$AlternateExecutable)) {
 if(-not $candidate){continue}
 [DeskWindowCheck]::PostMessage($window,0x10,[IntPtr]0,[IntPtr]0) | Out-Null
 Start-Sleep -Milliseconds 250
 if([DeskWindowCheck]::IsWindowVisible($window)){throw 'Expected hidden first window'}
 $second=Start-Process -FilePath $candidate -WorkingDirectory (Split-Path -Parent $candidate) -WindowStyle Hidden -PassThru
 if(-not $second.WaitForExit(10000)){throw "Duplicate process did not exit: $($second.Id)"}
 if($second.ExitCode -ne 0){throw "Duplicate exited abnormally: $($second.ExitCode)"}
 Start-Sleep -Milliseconds 250
 if(-not [DeskWindowCheck]::IsWindowVisible($window)){throw 'Existing instance was not restored'}
 Get-Process -Id $ProcessId | Out-Null
 Write-Output "PASS duplicate exits and restores original process $ProcessId : $candidate"
}
