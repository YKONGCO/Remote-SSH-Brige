param([int]$ProcessId,[switch]$CheckTray,[string]$Screenshot,[int]$Width=1440,[int]$Height=960)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;using System.Text;using System.Runtime.InteropServices;using System.Collections.Generic;
public class DeskWindowCheck {
 public delegate bool Callback(IntPtr h,IntPtr p);
 [StructLayout(LayoutKind.Sequential)] public struct RECT{public int L,T,R,B;}
 [DllImport("user32.dll")] public static extern bool EnumWindows(Callback c,IntPtr p);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr d,uint f);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int c);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr a,int x,int y,int w,int z,uint f);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h,int n);
 public static IntPtr Find(uint pid,string title,string cls){IntPtr result=IntPtr.Zero;EnumWindows((h,p)=>{uint owner;GetWindowThreadProcessId(h,out owner);if(owner==pid){var t=new StringBuilder(512);var c=new StringBuilder(512);GetWindowText(h,t,512);GetClassName(h,c,512);if((String.IsNullOrEmpty(title)||t.ToString()==title)&&(String.IsNullOrEmpty(cls)||c.ToString()==cls))result=h;}return true;},IntPtr.Zero);return result;}
}
'@
[DeskWindowCheck]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null
$window=[DeskWindowCheck]::Find($ProcessId,'远程管理',$null)
if($window -eq [IntPtr]0){throw 'Window not found'}
$style=[DeskWindowCheck]::GetWindowLong($window,-16)
Write-Output ('Window style: 0x{0:X}' -f $style)
[DeskWindowCheck]::ShowWindow($window,9) | Out-Null
[DeskWindowCheck]::SetWindowPos($window,[IntPtr]0,35,35,$Width,$Height,4) | Out-Null
if($CheckTray){
 $tray=[DeskWindowCheck]::Find($ProcessId,$null,'tray_icon_app')
 if($tray -eq [IntPtr]0){throw 'Tray window missing'}
 Write-Output 'PASS native tray icon created'
 [DeskWindowCheck]::PostMessage($window,0x10,[IntPtr]0,[IntPtr]0) | Out-Null
 Start-Sleep -Milliseconds 500
 if([DeskWindowCheck]::IsWindowVisible($window)){throw 'Close did not hide window'}
 Get-Process -Id $ProcessId | Out-Null
 Write-Output 'PASS close hides window and keeps process alive'
 [DeskWindowCheck]::PostMessage($tray,6002,[IntPtr]0,[IntPtr]0x202) | Out-Null
 Start-Sleep -Milliseconds 500
 if(-not [DeskWindowCheck]::IsWindowVisible($window)){throw 'Tray click did not restore window'}
 Write-Output 'PASS tray click restores window'
}
if($Screenshot){
 Start-Sleep -Milliseconds 500
 $r=New-Object DeskWindowCheck+RECT
 [DeskWindowCheck]::GetWindowRect($window,[ref]$r) | Out-Null
 $b=New-Object System.Drawing.Bitmap ($r.R-$r.L),($r.B-$r.T)
 $g=[System.Drawing.Graphics]::FromImage($b);$dc=$g.GetHdc()
 [DeskWindowCheck]::PrintWindow($window,$dc,2) | Out-Null
 $g.ReleaseHdc($dc);$b.Save($Screenshot,[System.Drawing.Imaging.ImageFormat]::Png);$g.Dispose();$b.Dispose()
 Write-Output "Captured $Screenshot"
}
