param([int]$RelX, [int]$RelY, [string]$Title = "AiCouncil", [int]$Steps = 12)
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class C2 {
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
[C2]::SetProcessDPIAware() | Out-Null
$script:target = [IntPtr]::Zero
$cb = {
    param($h, $lp)
    $len = [C2]::GetWindowTextLength($h)
    if ($len -gt 0 -and [C2]::IsWindowVisible($h)) {
        $sb = New-Object System.Text.StringBuilder ($len + 1)
        [void][C2]::GetWindowText($h, $sb, $sb.Capacity)
        if ($sb.ToString() -eq $Title) { $script:target = $h; return $false }
    }
    return $true
}
[C2]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($script:target -eq [IntPtr]::Zero) { Write-Error "not found"; exit 1 }
[C2]::SetForegroundWindow($script:target) | Out-Null
Start-Sleep -Milliseconds 600
$r = New-Object C2+RECT
[C2]::GetWindowRect($script:target, [ref]$r) | Out-Null
$tx = $r.L + $RelX; $ty = $r.T + $RelY
# 分步移动产生 hover 事件流
$cx = [C2]::GetCursorPos() # 不重要;直接线性插值
for ($i = 0; $i -le $Steps; $i++) {
    $px = $r.L + [int]($RelX * $i / $Steps)
    $py = $r.T + [int]($RelY * $i / $Steps)
    [void][C2]::SetCursorPos($px, $py)
    Start-Sleep -Milliseconds 30
}
[void][C2]::SetCursorPos($tx, $ty)
Start-Sleep -Milliseconds 250
[C2]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 60
[C2]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
Write-Output "clicked($tx,$ty)"
