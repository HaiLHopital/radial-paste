# Regenerates app.ico: a dark disc with a ring of eight snippet dots around an accent centre,
# the radial menu in miniature. Classic BMP entries for the small sizes and one PNG-compressed
# 256x256 entry. Needs nothing but .NET (System.Drawing); run from anywhere:
#   powershell -File make-icon.ps1
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

# Renders the motif at any size; every distance is a fraction of the 256x256 original.
function Render([int]$s) {
    $bmp = New-Object System.Drawing.Bitmap($s, $s)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    $centre = [single]($s / 2)
    $disc = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 32, 34, 40))
    $g.FillEllipse($disc, [single]($s * 6 / 256), [single]($s * 6 / 256), [single]($s * 244 / 256), [single]($s * 244 / 256))

    # Eight dots clockwise from 12 o'clock, like the ring's wedges; the first is the accent.
    $ring = [single]($s * 72 / 256)
    $dot = [single]($s * 24 / 256)
    for ($i = 0; $i -lt 8; $i++) {
        $angle = [Math]::PI * 2 * $i / 8 - [Math]::PI / 2
        $x = [single]($centre + [Math]::Cos($angle) * $ring - $dot / 2)
        $y = [single]($centre + [Math]::Sin($angle) * $ring - $dot / 2)
        $color = if ($i -eq 0) { [System.Drawing.Color]::FromArgb(255, 86, 182, 246) }
                 else { [System.Drawing.Color]::FromArgb(255, 235, 238, 242) }
        $brush = New-Object System.Drawing.SolidBrush($color)
        $g.FillEllipse($brush, $x, $y, $dot, $dot)
    }

    $centreBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 86, 182, 246))
    $centreDot = [single]($s * 40 / 256)
    $g.FillEllipse($centreBrush, [single]($centre - $centreDot / 2), [single]($centre - $centreDot / 2), $centreDot, $centreDot)
    return ,$bmp
}

# The classic uncompressed entry for one size: an ICO BITMAPINFOHEADER, the pixels bottom-up
# as BGRA, then the 1-bit transparency mask, rows padded to 4 bytes.
function BmpEntry([System.Drawing.Bitmap]$bmp) {
    $w = $bmp.Width
    $h = $bmp.Height
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)
    $bw.Write([uint32]40)            # BITMAPINFOHEADER size
    $bw.Write([int32]$w)
    $bw.Write([int32]($h * 2))       # XOR plus AND, as the ICO format wants
    $bw.Write([uint16]1)             # planes
    $bw.Write([uint16]32)            # bits per pixel
    $bw.Write([uint32]0)             # uncompressed
    $bw.Write([uint32](4 * $w * $h)) # XOR size
    $bw.Write([int32]0); $bw.Write([int32]0); $bw.Write([uint32]0); $bw.Write([uint32]0)
    for ($y = $h - 1; $y -ge 0; $y--) {
        for ($x = 0; $x -lt $w; $x++) {
            $c = $bmp.GetPixel($x, $y)
            $bw.Write([byte]$c.B); $bw.Write([byte]$c.G); $bw.Write([byte]$c.R); $bw.Write([byte]$c.A)
        }
    }
    $rowBytes = [int][Math]::Ceiling($w / 32.0) * 4
    for ($y = $h - 1; $y -ge 0; $y--) {
        $row = New-Object byte[] $rowBytes
        for ($x = 0; $x -lt $w; $x++) {
            if ($bmp.GetPixel($x, $y).A -eq 0) {
                # A shift, not a division: the [int] cast rounds, and the last pixel of a
                # 32-wide row would land one index past the end.
                $row[$x -shr 3] = $row[$x -shr 3] -bor [byte](128 -shr ($x -band 7))
            }
        }
        $bw.Write($row)
    }
    return ,$ms.ToArray()
}

$images = @()
foreach ($s in 16, 32, 48) {
    $images += ,@{ width = $s; png = $false; bytes = BmpEntry (Render $s) }
}

$size = 256
$stream = New-Object System.IO.MemoryStream
Render $size | ForEach-Object {
    $g = [System.Drawing.Graphics]::FromImage($_) | Out-Null
    $_.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
}
$png = $stream.ToArray()
$images += ,@{ width = $size; png = $true; bytes = $png }

# The ICO container: a 6-byte header, one 16-byte directory entry per image, then the images.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($out)
$w.Write([uint16]0)              # reserved
$w.Write([uint16]1)              # type: icon
$w.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($image in $images) {
    $edge = [byte]($image.width % 256)
    $w.Write([byte]$edge); $w.Write([byte]$edge)
    $w.Write([byte]0)            # palette colours
    $w.Write([byte]0)            # reserved
    $w.Write([uint16]1)          # colour planes
    $w.Write([uint16]32)         # bits per pixel
    $w.Write([uint32]$image.bytes.Length)
    $w.Write([uint32]$offset)
    $offset += $image.bytes.Length
}
foreach ($image in $images) {
    $w.Write($image.bytes)
}

$dest = Join-Path $PSScriptRoot 'app.ico'
[System.IO.File]::WriteAllBytes($dest, $out.ToArray())
"wrote $dest ($([System.IO.FileInfo]::new($dest).Length) bytes, $($images.Count) images)"
