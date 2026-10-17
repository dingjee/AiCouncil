param([string]$In, [string]$Out, [int]$X, [int]$Y, [int]$W, [int]$H)
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::FromFile($In)
$rect = New-Object System.Drawing.Rectangle($X, $Y, $W, $H)
$crop = $bmp.Clone($rect, $bmp.PixelFormat)
$crop.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$crop.Dispose(); $bmp.Dispose()
Write-Output "saved $Out"
