param([string]$Title = "AiCouncil", [string]$OutFile = "shot.png", [int]$WaitMs = 1500)
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class WD {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public delegate bool EnumProc(IntPtr h, IntPtr lp);
}
"@
Add-Type -AssemblyName System.Drawing
[WD]::SetProcessDPIAware() | Out-Null
$target = [IntPtr]::Zero
$cb = {
    param($h, $lp)
    $len = [WD]::GetWindowTextLength($h)
    if ($len -gt 0 -and [WD]::IsWindowVisible($h)) {
        $sb = New-Object System.Text.StringBuilder ($len + 1)
        [void][WD]::GetWindowText($h, $sb, $sb.Capacity)
        if ($sb.ToString() -eq $Title) { $script:target = $h; return $false }
    }
    return $true
}
[WD]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($target -eq [IntPtr]::Zero) { Write-Error "not found"; exit 1 }
Start-Sleep -Milliseconds $WaitMs
$r = New-Object WD+RECT
[WD]::GetWindowRect($target, [ref]$r) | Out-Null
$w = $r.R - $r.L; $h2 = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap($w, $h2)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[WD]::PrintWindow($target, $hdc, 2) | Out-Null
$g.ReleaseHdc($hdc)
$bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $OutFile ($w x $h2)"
