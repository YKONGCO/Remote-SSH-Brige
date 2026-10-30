param([int]$ProcessId,[string]$ReportPath)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
& (Join-Path $PSScriptRoot 'window-check.ps1') -ProcessId $ProcessId -Width 1440 -Height 900
$window=[DeskWindowCheck]::Find($ProcessId,'远程管理',$null)
function Find-Control($root,[string]$name,$type) {
 $nameCondition=New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::NameProperty),$name
 $typeCondition=New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ControlTypeProperty),$type
 $condition=New-Object System.Windows.Automation.AndCondition $nameCondition,$typeCondition
 return $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants,$condition)
}
function Get-Root { return [System.Windows.Automation.AutomationElement]::FromHandle($window) }
function Navigate([string]$name) {
 $button=$null
 for($attempt=0;$attempt -lt 20;$attempt++){
  $rootElement=Get-Root
  $button=Find-Control $rootElement $name ([System.Windows.Automation.ControlType]::Button)
  if($button){break};Start-Sleep -Milliseconds 150
 }
 if(-not $button){throw "Navigation missing: $name"}
 ([System.Windows.Automation.InvokePattern]$button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)).Invoke()
 Start-Sleep -Milliseconds 400
}
$report=@()
foreach($page in @('操作记录','设置')) {
 Navigate $page
 $rootElement=Get-Root
 $heading=Find-Control $rootElement $page ([System.Windows.Automation.ControlType]::Text)
 if(-not $heading){throw "Heading missing: $page"}
 $before=$heading.Current.BoundingRectangle.Top
 $tabs=$null;$tableHeader=$null
 if($page -eq '操作记录') {
  $tabs=Find-Control $rootElement '记录类型' ([System.Windows.Automation.ControlType]::Tab)
  $tableHeader=Find-Control $rootElement '时间' ([System.Windows.Automation.ControlType]::DataItem)
  if(-not $tabs -or -not $tableHeader){throw 'Audit filters or table header missing'}
  $tabsTop=$tabs.Current.BoundingRectangle.Top;$tableTop=$tableHeader.Current.BoundingRectangle.Top
 }
 $elements=$rootElement.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)
 $candidates=@($elements | Where-Object { if($page -eq '操作记录'){ $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::DataItem -and $_.Current.IsKeyboardFocusable }else{ $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Text -and $_.Current.Name -ne '设置' } })
 if($candidates.Count -lt 2){throw "Expected scroll targets: $page"}
 $first=[System.Windows.Automation.ScrollItemPattern]$candidates[0].GetCurrentPattern([System.Windows.Automation.ScrollItemPattern]::Pattern)
 $first.ScrollIntoView();Start-Sleep -Milliseconds 150
 $target=$candidates[-1]
 $targetBefore=$target.Current.BoundingRectangle.Top
 ([System.Windows.Automation.ScrollItemPattern]$target.GetCurrentPattern([System.Windows.Automation.ScrollItemPattern]::Pattern)).ScrollIntoView()
 Start-Sleep -Milliseconds 350
 $targetAfter=$target.Current.BoundingRectangle.Top
 if([Math]::Abs($targetBefore-$targetAfter) -lt 5){throw "Content did not scroll: $page"}
 $after=$heading.Current.BoundingRectangle.Top
 if([Math]::Abs($before-$after) -gt 1){throw "Header moved when scrolling: $page"}
 if($tabs -and [Math]::Abs($tabs.Current.BoundingRectangle.Top-$tabsTop) -gt 1){throw 'Audit filters moved'}
 if($tableHeader -and [Math]::Abs($tableHeader.Current.BoundingRectangle.Top-$tableTop) -gt 1){throw 'Table header moved'}
 $report += [PSCustomObject]@{page=$page;headingTopBefore=$before;headingTopAfter=$after;targetTopBefore=$targetBefore;targetTopAfter=$targetAfter;filtersFixed=($null -ne $tabs);tableHeaderFixed=($null -ne $tableHeader)}
 Write-Output "PASS $page : header fixed after scrolling to end"
}
Navigate '终端会话'
$rootElement=Get-Root
$buttons=$rootElement.FindAll([System.Windows.Automation.TreeScope]::Descendants,(New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ControlTypeProperty),([System.Windows.Automation.ControlType]::Button)))
$createButtons=@($buttons | Where-Object {$_.Current.Name -eq '新建终端'})
if($createButtons.Count -ne 1){throw 'Expected one terminal create action'}
$status=Find-Control $rootElement '等待连接' ([System.Windows.Automation.ControlType]::Text)
$rect=New-Object DeskWindowCheck+RECT
[DeskWindowCheck]::GetWindowRect($window,[ref]$rect) | Out-Null
if(-not $status -or $status.Current.BoundingRectangle.Bottom -gt $rect.B){throw 'Terminal status is clipped'}
Write-Output 'PASS terminal toolbar has one create action and status fits in viewport'
if($ReportPath){$report | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath $ReportPath}
