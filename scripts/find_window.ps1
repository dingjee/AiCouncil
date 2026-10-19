param([string]$Title = "AiCouncil")
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class WEnum {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public delegate bool EnumProc(IntPtr h, IntPtr lp);
}
"@
$found = @()
$cb = {
    param($h, $lp)
    $len = [WEnum]::GetWindowTextLength($h)
    if ($len -gt 0) {
        $sb = New-Object System.Text.StringBuilder ($len + 1)
        [void][WEnum]::GetWindowText($h, $sb, $sb.Capacity)
        $title = $sb.ToString()
        $pid2 = 0
        [void][WEnum]::GetWindowThreadProcessId($h, [ref]$pid2)
        $r = New-Object WEnum+RECT
        [void][WEnum]::GetWindowRect($h, [ref]$r)
        $script:found += [pscustomobject]@{
            Hwnd = $h; Title = $title; Pid = $pid2
            Vis = [WEnum]::IsWindowVisible($h)
            Rect = "$($r.L),$($r.T) $($r.R - $r.L)x$($r.B - $r.T)"
        }
    }
    return $true
}
[WEnum]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
$found | Where-Object { $_.Title -like "*$Title*" -or $_.Pid -eq (Get-Process aicouncil -ErrorAction SilentlyContinue).Id } | Format-Table -AutoSize
