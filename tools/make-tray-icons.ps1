# Builds the two tray icons from the project's own logo (assets/logo-white-src.png):
#   assets/tray-on-dark.ico   white glyph, for a dark taskbar
#   assets/tray-on-light.ico  near-black glyph, for a light taskbar
# Everything darker than the logo's white plate becomes the glyph, with its anti-aliasing kept
# as transparency; the orange eye keeps its colour. The mask is made at full resolution, then
# scaled down once per size, so small sizes stay smooth. Each .ico holds PNG entries at every
# small-icon size Windows asks for (100 % to 300 % scaling) plus 64 px.
#
#   powershell -ExecutionPolicy Bypass -File tools\make-tray-icons.ps1 [-Preview out.png]
param([string]$Preview = "")
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing @"
using System; using System.Drawing; using System.Drawing.Imaging; using System.Runtime.InteropServices;
public static class Glyph {
    const int Corner = 220, Edge = 8;
    // Full-size glyph: ink colour with alpha from how dark the pixel is, orange kept as is.
    public static Bitmap Mask(Bitmap src, Color ink, out Rectangle box) {
        int w = src.Width, h = src.Height;
        var rect = new Rectangle(0, 0, w, h);
        var input = src.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        var buf = new byte[w * h * 4];
        Marshal.Copy(input.Scan0, buf, 0, buf.Length);
        src.UnlockBits(input);
        int minX = w, minY = h, maxX = -1, maxY = -1;
        for (int y = 0; y < h; y++) for (int x = 0; x < w; x++) {
            int i = (y * w + x) * 4;
            byte b = buf[i], g = buf[i + 1], r = buf[i + 2], a0 = buf[i + 3];
            // Transparent corners count as the white plate.
            double lum = a0 == 0 ? 255 : 0.299 * r + 0.587 * g + 0.114 * b;
            bool orange = a0 > 0 && r > 180 && g < 150 && b < 90;
            int a = orange ? 255 : (int)Math.Max(0, Math.Min(255, (250 - lum) * 10));
            // The plate's thin rounded outline lives in the four corners; the glyph stays
            // well clear of them and of the edges.
            bool cornerX = x < Corner || x >= w - Corner, cornerY = y < Corner || y >= h - Corner;
            if ((cornerX && cornerY) || x < Edge || y < Edge || x >= w - Edge || y >= h - Edge) { orange = false; a = 0; }
            if (!orange) { buf[i] = ink.B; buf[i + 1] = ink.G; buf[i + 2] = ink.R; }
            buf[i + 3] = (byte)a;
            if (a > 8) { minX = Math.Min(minX, x); maxX = Math.Max(maxX, x); minY = Math.Min(minY, y); maxY = Math.Max(maxY, y); }
        }
        var dst = new Bitmap(w, h, PixelFormat.Format32bppArgb);
        var output = dst.LockBits(rect, ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
        Marshal.Copy(buf, 0, output.Scan0, buf.Length);
        dst.UnlockBits(output);
        box = Rectangle.FromLTRB(minX, minY, maxX + 1, maxY + 1);
        return dst;
    }
    // One size: the glyph's bounding square, with a small margin, scaled down in one pass.
    public static Bitmap Scale(Bitmap mask, Rectangle box, int size) {
        double side = Math.Max(box.Width, box.Height) * 1.06;
        double cx = box.X + box.Width / 2.0, cy = box.Y + box.Height / 2.0;
        var from = new RectangleF((float)(cx - side / 2), (float)(cy - side / 2), (float)side, (float)side);
        var pre = new Bitmap(size, size, PixelFormat.Format32bppPArgb);
        using (var g = Graphics.FromImage(pre)) {
            g.Clear(Color.Transparent);
            g.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = System.Drawing.Drawing2D.PixelOffsetMode.HighQuality;
            g.CompositingMode = System.Drawing.Drawing2D.CompositingMode.SourceCopy;
            using (var attrs = new ImageAttributes()) {
                attrs.SetWrapMode(System.Drawing.Drawing2D.WrapMode.TileFlipXY);
                g.DrawImage(mask, new Rectangle(0, 0, size, size), from.X, from.Y, from.Width, from.Height, GraphicsUnit.Pixel, attrs);
            }
        }
        var bmp = new Bitmap(size, size, PixelFormat.Format32bppArgb);
        using (var g = Graphics.FromImage(bmp)) { g.CompositingMode = System.Drawing.Drawing2D.CompositingMode.SourceCopy; g.DrawImage(pre, 0, 0); }
        pre.Dispose();
        return bmp;
    }
}
"@

function Write-Ico([System.Drawing.Bitmap[]]$bmps, [string]$path) {
    $pngs = foreach ($b in $bmps) { $ms = New-Object IO.MemoryStream; $b.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); , $ms.ToArray() }
    $out = New-Object IO.MemoryStream
    $w = New-Object IO.BinaryWriter $out
    $w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$bmps.Count)
    $offset = 6 + 16 * $bmps.Count
    for ($k = 0; $k -lt $bmps.Count; $k++) {
        $s = $bmps[$k].Width
        $w.Write([byte]($s % 256)); $w.Write([byte]($s % 256)); $w.Write([byte]0); $w.Write([byte]0)
        $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$pngs[$k].Length); $w.Write([uint32]$offset)
        $offset += $pngs[$k].Length
    }
    foreach ($p in $pngs) { $w.Write($p) }
    [IO.File]::WriteAllBytes($path, $out.ToArray())
}

$assets = Join-Path $PSScriptRoot "..\assets"
$src = [System.Drawing.Bitmap]::FromFile((Resolve-Path (Join-Path $assets "logo-white-src.png")))
$sizes = 16, 20, 24, 28, 32, 36, 40, 48, 64
$box = [System.Drawing.Rectangle]::Empty
$white = [Glyph]::Mask($src, [System.Drawing.Color]::FromArgb(255, 255, 255), [ref]$box)
$black = [Glyph]::Mask($src, [System.Drawing.Color]::FromArgb(28, 28, 28), [ref]$box)
$onDark = foreach ($s in $sizes) { [Glyph]::Scale($white, $box, $s) }
$onLight = foreach ($s in $sizes) { [Glyph]::Scale($black, $box, $s) }
Write-Ico $onDark (Join-Path $assets "tray-on-dark.ico")
Write-Ico $onLight (Join-Path $assets "tray-on-light.ico")

if ($Preview) {
    # Each size on both taskbar colours, at 1:1 and enlarged x4 without smoothing.
    $pv = New-Object System.Drawing.Bitmap 1080, 560
    $g = [System.Drawing.Graphics]::FromImage($pv)
    $g.FillRectangle((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(32, 32, 32))), 0, 0, 1080, 280)
    $g.FillRectangle((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(243, 243, 243))), 0, 280, 1080, 280)
    $g.InterpolationMode = 'NearestNeighbor'; $g.PixelOffsetMode = 'Half'
    $x = 10
    for ($k = 0; $k -lt 8; $k++) {
        $s = $sizes[$k]
        $g.DrawImage($onDark[$k], $x, 10, $s, $s); $g.DrawImage($onDark[$k], $x, 70, $s * 4, $s * 4)
        $g.DrawImage($onLight[$k], $x, 290, $s, $s); $g.DrawImage($onLight[$k], $x, 350, $s * 4, $s * 4)
        $x += [Math]::Max($s * 4, 40) - 0 + 10
        if ($x -gt 1000) { break }
    }
    $pv.Save($Preview)
}
"wrote tray-on-dark.ico and tray-on-light.ico ($($sizes -join ', ') px), glyph box $box"
