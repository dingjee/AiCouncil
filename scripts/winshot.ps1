# winshot.ps1 — 把 aicouncil 窗口放到副屏(若有)并按窗口矩形截图。
# 用法:powershell -File scripts\winshot.ps1 -OutPath <png> [-MoveOnly]
#   -MoveOnly: 只移动窗口不截图(启动后立即调用,减少主屏打扰)
# 行为:
#   1) 等待 aicouncil 主窗口出现(最多 10s);
#   2) 若存在非主显示器,把窗口移动到第一个非主屏(SetWindowPos),
#      避免弹在 1 号屏抢焦点/挡工作;
#   3) 置前并按窗口物理像素矩形截图(DPI aware)。
param(
    [string]$OutPath,
    [switch]$MoveOnly
)
if (-not $MoveOnly -and -not $OutPath) { Write-Error "OutPath required (or pass -MoveOnly)"; exit 1 }

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
}
"@
[Win]::SetProcessDPIAware() | Out-Null

# 1) 等待主窗口
$proc = $null
for ($i = 0; $i -lt 100; $i++) {
    $proc = Get-Process aicouncil -ErrorAction SilentlyContinue |
        Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    if ($proc) { break }
    Start-Sleep -Milliseconds 100
}
if (-not $proc) { Write-Error "aicouncil window not found"; exit 1 }
$h = $proc.MainWindowHandle

# 2) 有副屏则移动过去(只移动一次)
$screens = [System.Windows.Forms.Screen]::AllScreens
$side = $screens | Where-Object { -not $_.Primary } | Select-Object -First 1
if ($side) {
    $b = $side.Bounds
    $r = New-Object Win+RECT
    [Win]::GetWindowRect($h, [ref]$r) | Out-Null
    $w = $r.R - $r.L; $ht = $r.B - $r.T
    if ($w -gt 0 -and $ht -gt 0) {
        # SWP_NOZORDER=0x4 | SWP_SHOWWINDOW=0x40
        [Win]::SetWindowPos($h, [IntPtr]::Zero, $b.X + 60, $b.Y + 60, $w, $ht, 0x4 -bor 0x40) | Out-Null
    }
    Write-Output "moved to side display $($b.Size)"
} else {
    Write-Output "no side display; staying on primary"
}

if ($MoveOnly) { exit 0 }

# 3) 置前 + 等一帧 + 按窗口矩形截图
[Win]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 400
$r = New-Object Win+RECT
[Win]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.R - $r.L; $ht = $r.B - $r.T
if ($w -le 0 -or $ht -le 0) { Write-Error "bad window rect"; exit 1 }
$bmp = New-Object System.Drawing.Bitmap($w, $ht)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object System.Drawing.Size($w, $ht)))
$bmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $OutPath ${w}x${ht}"
