param(
    [string]$ProcessName = "render_probe",
    [string]$OutPrefix = "page",
    [int]$Pages = 5
)
# 翻页截取:每次 PageDown 后截一张窗口图
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32 {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$deadline = (Get-Date).AddSeconds(20)
do {
    Start-Sleep -Milliseconds 500
    $proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
} while (-not $proc -and (Get-Date) -lt $deadline)
if (-not $proc) { Write-Error "window not found"; exit 1 }
$hwnd = $proc.MainWindowHandle

function Shoot([string]$file) {
    $r = New-Object Win32+RECT
    [Win32]::GetWindowRect($hwnd, [ref]$r) | Out-Null
    $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
    $bmp = New-Object System.Drawing.Bitmap($w, $h)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [Win32]::PrintWindow($hwnd, $hdc, 2) | Out-Null
    $g.ReleaseHdc($hdc)
    $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

[Win32]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds 1500
for ($i = 0; $i -lt $Pages; $i++) {
    Shoot "$OutPrefix$i.png"
    Write-Output "saved $OutPrefix$i.png"
    [System.Windows.Forms.SendKeys]::SendWait("{PGDN 6}")
    Start-Sleep -Milliseconds 700
}
