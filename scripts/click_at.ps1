param([int]$RelX, [int]$RelY, [string]$Title = "AiCouncil")
# 相对窗口客户区的 DPI 真实像素坐标点击
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class CW {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public delegate bool EnumProc(IntPtr h, IntPtr lp);
}
"@
[CW]::SetProcessDPIAware() | Out-Null
$target = [IntPtr]::Zero
$cb = {
    param($h, $lp)
    $len = [CW]::GetWindowTextLength($h)
    if ($len -gt 0 -and [CW]::IsWindowVisible($h)) {
        $sb = New-Object System.Text.StringBuilder ($len + 1)
        [void][CW]::GetWindowText($h, $sb, $sb.Capacity)
        if ($sb.ToString() -eq $Title) { $script:target = $h; return $false }
    }
    return $true
}
[CW]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($target -eq [IntPtr]::Zero) { Write-Error "not found"; exit 1 }
[CW]::SetForegroundWindow($target) | Out-Null
Start-Sleep -Milliseconds 400
$r = New-Object CW+RECT
[CW]::GetWindowRect($target, [ref]$r) | Out-Null
$x = $r.L + $RelX
$y = $r.T + $RelY
[CW]::SetCursorPos($x, $y) | Out-Null
Start-Sleep -Milliseconds 150
[CW]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
[CW]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
Write-Output "clicked abs($x,$y) rel($RelX,$RelY) win($($r.L),$($r.T))"
