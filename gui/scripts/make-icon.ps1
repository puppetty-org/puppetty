# Generates icon-source.png (1024x1024): a marionette crossbar with strings
# puppeteering a terminal prompt — "Puppeteer for terminals".
Add-Type -AssemblyName System.Drawing

function New-RoundedPath([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

$size = 1024
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

# Background: dark rounded tile with subtle vertical gradient + border
$bg = New-RoundedPath 32 32 960 960 190
$grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0, 0)),
    (New-Object System.Drawing.Point(0, 1024)),
    [System.Drawing.Color]::FromArgb(255, 34, 38, 48),
    [System.Drawing.Color]::FromArgb(255, 14, 16, 20))
$g.FillPath($grad, $bg)
$borderPen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 58, 64, 78), 10)
$g.DrawPath($borderPen, $bg)

# Marionette crossbar
$barBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 141, 151, 168))
$bar = New-RoundedPath 292 104 440 44 22
$g.FillPath($barBrush, $bar)

# Strings from crossbar to the chevron joints and the cursor
$stringPen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 84, 92, 110), 11)
$g.DrawLine($stringPen, 352, 148, 330, 366)
$g.DrawLine($stringPen, 512, 148, 538, 520)
$g.DrawLine($stringPen, 672, 148, 690, 622)

# Terminal chevron (the puppet)
$chevPen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 122, 162, 247), 100)
$chevPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$chevPen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$chevPen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
[System.Drawing.Point[]]$pts = @(
    (New-Object System.Drawing.Point(330, 366)),
    (New-Object System.Drawing.Point(538, 520)),
    (New-Object System.Drawing.Point(330, 674))
)
$g.DrawLines($chevPen, $pts)

# Cursor block (also on a string)
$curBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 224, 175, 104))
$cur = New-RoundedPath 600 622 190 86 22
$g.FillPath($curBrush, $cur)

$g.Dispose()
$out = Join-Path $PSScriptRoot "..\src-tauri\icon-source.png"
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "wrote $out"
