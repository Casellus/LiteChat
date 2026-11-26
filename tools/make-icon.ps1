# Regenerates the LiteChat icon (white feather in a round speech bubble on a green circle) into assets/:
#   litechat.ico (exe), icon64.rgba (window and tray), litechat.png (preview)
#   powershell -ExecutionPolicy Bypass -File tools\make-icon.ps1
param($outDir = (Join-Path $PSScriptRoot '..\assets'))
$outDir = (Resolve-Path $outDir).Path
Add-Type -AssemblyName System.Drawing
function DibBytes($b) {
  $s = $b.Width; $ms = New-Object IO.MemoryStream; $w = New-Object IO.BinaryWriter $ms
  $w.Write([uint32]40); $w.Write([int32]$s); $w.Write([int32]($s*2)); $w.Write([uint16]1); $w.Write([uint16]32)
  $w.Write([uint32]0); $w.Write([uint32]0); $w.Write([int32]0); $w.Write([int32]0); $w.Write([uint32]0); $w.Write([uint32]0)
  for ($y=$s-1; $y -ge 0; $y--) { for ($x=0; $x -lt $s; $x++) { $c=$b.GetPixel($x,$y); $w.Write([byte]$c.B); $w.Write([byte]$c.G); $w.Write([byte]$c.R); $w.Write([byte]$c.A) } }
  $rowBytes = [int]([Math]::Ceiling($s/32.0)*4); $w.Write((New-Object byte[] ($rowBytes*$s)))
  $w.Flush(); ,$ms.ToArray()
}
function Draw([int]$S) {
  $bmp = New-Object Drawing.Bitmap $S, $S, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'; $g.PixelOffsetMode = 'HighQuality'; $g.Clear([Drawing.Color]::Transparent)
  $k = $S / 256.0
  function P($x,$y){ New-Object Drawing.PointF ([single]($x*$k)), ([single]($y*$k)) }
  # feather coordinates are drawn scaled into the bubble
  function F($x,$y){ P (130 + ($x-129)*0.56) (124 + ($y-117)*0.56) }
  # round green background
  $rect = New-Object Drawing.RectangleF 0, 0, $S, $S
  $c1 = [Drawing.Color]::FromArgb(255,0x4A,0xDE,0x80); $c2 = [Drawing.Color]::FromArgb(255,0x0B,0x8A,0x5A)
  $grad = New-Object Drawing.Drawing2D.LinearGradientBrush $rect, $c1, $c2, ([single]60)
  $g.FillEllipse($grad, [single](4*$k), [single](4*$k), [single](248*$k), [single](248*$k))
  # round speech bubble outline with a tail at the bottom left
  $r = 78; $cx = 130; $cy = 124
  $ring = New-Object Drawing.Drawing2D.GraphicsPath
  $ring.AddArc([single](($cx-$r)*$k), [single](($cy-$r)*$k), [single](2*$r*$k), [single](2*$r*$k), 150, 330)
  $ring.AddLine((P 91 191.5), (P 52 214))
  $ring.AddLine((P 52 214), (P 62.5 163))
  $ring.CloseFigure()
  $pen = New-Object Drawing.Pen ([Drawing.Brushes]::White), ([single]([Math]::Max(1.6, 15*$k)))
  $pen.LineJoin = 'Round'
  $g.DrawPath($pen, $ring)
  # feather: tilted drop-shaped vane
  $f = New-Object Drawing.Drawing2D.GraphicsPath
  $f.AddBezier((F 78 176),(F 70 110),(F 130 52),(F 196 40))
  $f.AddBezier((F 196 40),(F 190 104),(F 150 164),(F 78 176))
  $f.CloseFigure()
  $g.FillPath([Drawing.Brushes]::White, $f)
  $cut = New-Object Drawing.SolidBrush ([Drawing.Color]::FromArgb(255,0x22,0xB0,0x6E))
  if ($S -ge 32) {
    # side notches and shaft
    $g.FillPolygon($cut, [Drawing.PointF[]]@((F 94 108),(F 118 112),(F 92 124)))
    $g.FillPolygon($cut, [Drawing.PointF[]]@((F 172 118),(F 148 120),(F 166 104)))
    $sp = New-Object Drawing.Pen $cut, ([single]([Math]::Max(1.2, 7*0.56*$k)))
    $sp.StartCap = 'Round'; $sp.EndCap = 'Round'
    $g.DrawBezier($sp, (F 182 56), (F 150 92), (F 116 134), (F 84 170))
  }
  $pw = New-Object Drawing.Pen ([Drawing.Brushes]::White), ([single]([Math]::Max(1.2, 9*0.56*$k)))
  $pw.StartCap = 'Round'; $pw.EndCap = 'Round'
  $g.DrawLine($pw, (F 84 170), (F 60 196))
  $g.Dispose(); $bmp
}
# ICO: BMP entries for small sizes, PNG for 256 px
$sizes = 16,24,32,48,64,128,256
$pngs = foreach ($s in $sizes) { $b = Draw $s; if ($s -lt 256) { ,(DibBytes $b) } else { $ms = New-Object IO.MemoryStream; $b.Save($ms, [Drawing.Imaging.ImageFormat]::Png); ,$ms.ToArray() } }
$fs = [IO.File]::Create("$outDir\litechat.ico"); $w = New-Object IO.BinaryWriter $fs
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$off = 6 + 16*$sizes.Count
for ($i=0; $i -lt $sizes.Count; $i++) { $s=$sizes[$i]; $d=$pngs[$i]; $w.Write([byte]($s % 256)); $w.Write([byte]($s % 256)); $w.Write([byte]0); $w.Write([byte]0); $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$d.Length); $w.Write([uint32]$off); $off += $d.Length }
foreach ($d in $pngs) { $w.Write($d) }; $w.Close()
# raw 64x64 RGBA for the window and tray
$b = Draw 64; $raw = New-Object byte[] (64*64*4)
for ($y=0;$y -lt 64;$y++){ for($x=0;$x -lt 64;$x++){ $c=$b.GetPixel($x,$y); $i=($y*64+$x)*4; $raw[$i]=$c.R; $raw[$i+1]=$c.G; $raw[$i+2]=$c.B; $raw[$i+3]=$c.A } }
[IO.File]::WriteAllBytes("$outDir\icon64.rgba", $raw)
(Draw 256).Save("$outDir\litechat.png", [Drawing.Imaging.ImageFormat]::Png)
