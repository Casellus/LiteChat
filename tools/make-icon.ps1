# Regenerates the LiteChat icon (white feather in a green bubble) into assets/:
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
  # bubble: rounded rectangle + tail at the bottom left
  $r = 64; $x0=12; $y0=12; $x1=244; $y1=214
  $bub = New-Object Drawing.Drawing2D.GraphicsPath
  $bub.AddArc([single]($x0*$k),[single]($y0*$k),[single](2*$r*$k),[single](2*$r*$k),180,90)
  $bub.AddArc([single](($x1-2*$r)*$k),[single]($y0*$k),[single](2*$r*$k),[single](2*$r*$k),270,90)
  $bub.AddArc([single](($x1-2*$r)*$k),[single](($y1-2*$r)*$k),[single](2*$r*$k),[single](2*$r*$k),0,90)
  $bub.AddLine((P 110 $y1),(P 72 248))
  $bub.AddLine((P 72 248),(P 70 $y1))
  $bub.AddArc([single]($x0*$k),[single](($y1-2*$r)*$k),[single](2*$r*$k),[single](2*$r*$k),90,90)
  $bub.CloseFigure()
  $rect = New-Object Drawing.RectangleF 0, 0, $S, $S
  $c1 = [Drawing.Color]::FromArgb(255,0x4A,0xDE,0x80); $c2 = [Drawing.Color]::FromArgb(255,0x0B,0x8A,0x5A)
  $grad = New-Object Drawing.Drawing2D.LinearGradientBrush $rect, $c1, $c2, ([single]60)
  $g.FillPath($grad, $bub)
  # feather: tilted drop-shaped vane
  $f = New-Object Drawing.Drawing2D.GraphicsPath
  $f.AddBezier((P 78 176),(P 70 110),(P 130 52),(P 196 40))
  $f.AddBezier((P 196 40),(P 190 104),(P 150 164),(P 78 176))
  $f.CloseFigure()
  $g.FillPath([Drawing.Brushes]::White, $f)
  # side notches of the feather (background colour)
  $cut = New-Object Drawing.SolidBrush ([Drawing.Color]::FromArgb(255,0x1F,0xA8,0x6A))
  $g.FillPolygon($cut, [Drawing.PointF[]]@((P 94 108),(P 116 112),(P 92 122)))
  $g.FillPolygon($cut, [Drawing.PointF[]]@((P 172 118),(P 150 120),(P 166 106)))
  # shaft (quill) coming out at the bottom
  if ($S -ge 32) {
    $pen = New-Object Drawing.Pen $cut, ([single]([Math]::Max(1.2, 7*$k)))
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'
    $g.DrawBezier($pen, (P 182 56), (P 150 92), (P 116 134), (P 84 170))
  }
  $pw = New-Object Drawing.Pen ([Drawing.Brushes]::White), ([single]([Math]::Max(1.5, 9*$k)))
  $pw.StartCap = 'Round'; $pw.EndCap = 'Round'
  $g.DrawLine($pw, (P 84 170), (P 62 194))
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
