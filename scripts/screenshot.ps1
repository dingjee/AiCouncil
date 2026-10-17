param([string]$OutFile = "shot.png", [int]$DelayMs = 500)
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Start-Sleep -Milliseconds $DelayMs
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
$bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $OutFile"
