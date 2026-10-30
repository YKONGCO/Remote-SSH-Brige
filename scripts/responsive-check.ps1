param([int]$ProcessId,[string]$ReportPath)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
& (Join-Path $PSScriptRoot 'window-check.ps1') -ProcessId $ProcessId -Width 1440 -Height 960
Add-Type @'
using System;using System.Runtime.InteropServices;
public class ResponsiveDpi { [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd); }
'@
$window=[DeskWindowCheck]::Find($ProcessId,([string][char]0x8fdc+[char]0x7a0b+[char]0x7ba1+[char]0x7406),$null)
$report=@()
foreach($physicalWidth in @(1200,1440,1800)) {
 [DeskWindowCheck]::SetWindowPos($window,[IntPtr]0,35,35,$physicalWidth,960,4) | Out-Null
 Start-Sleep -Milliseconds 350
 $rootElement=[System.Windows.Automation.AutomationElement]::FromHandle($window)
 $elements=$rootElement.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)
 $horizontal=@()
 foreach($element in $elements) {
  $pattern=$null
  if($element.TryGetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern,[ref]$pattern)) {
   if(([System.Windows.Automation.ScrollPattern]$pattern).Current.HorizontallyScrollable) { $horizontal += $element.Current.Name }
  }
 }
 if($horizontal.Count -gt 0){throw "Unexpected horizontal scrolling at $physicalWidth : $($horizontal -join ',')"}
 $rect=New-Object DeskWindowCheck+RECT
 [DeskWindowCheck]::GetWindowRect($window,[ref]$rect) | Out-Null
 $timeHeader=$elements | Where-Object { $_.Current.Name -eq ([string][char]0x65f6+[char]0x95f4) } | Select-Object -First 1
 if(-not $timeHeader){throw 'Time column missing'}
 if($timeHeader.Current.BoundingRectangle.Right -gt $rect.R){throw 'Time column clipped outside window'}
 $scale=[ResponsiveDpi]::GetDpiForWindow($window)/96
 $report += [PSCustomObject]@{physicalWidth=$physicalWidth;contentWidth=[Math]::Round(($rect.R-$rect.L)/$scale);horizontalOverflow=$false;timeColumnVisible=$true}
 Write-Output "PASS audit layout at $physicalWidth physical pixels; no horizontal overflow and time column fits"
}
if($ReportPath){$report | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath $ReportPath}
