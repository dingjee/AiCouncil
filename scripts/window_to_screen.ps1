# 把 AiCouncil 窗口移到 (10,10) 并缩到 1500x900(物理像素)
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class WM {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int ht, bool repaint);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public delegate bool EnumProc(IntPtr h, IntPtr lp);
}
"@
[WM]::SetProcessDPIAware() | Out-Null
$target = [IntPtr]::Zero
$cb = {
    param($h, $lp)
    $len = [WM]::GetWindowTextLength($h)
    if ($len -gt 0 -and [WM]::IsWindowVisible($h)) {
        $sb = New-Object System.Text.StringBuilder ($len + 1)
        [void][WM]::GetWindowText($h, $sb, $sb.Capacity)
        if ($sb.ToString() -eq "AiCouncil") { $script:target = $h; return $false }
    }
    return $true
}
[WM]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($target -eq [IntPtr]::Zero) { Write-Error "not found"; exit 1 }
[WM]::MoveWindow($target, 5, 5, 1500, 900, $true) | Out-Null
Write-Output "moved"
