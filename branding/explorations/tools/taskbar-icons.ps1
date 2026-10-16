# Collects the inputs for taskbar_sheet.py from this machine, without changing any setting:
#   - the icons of the apps pinned to the taskbar, as the shell renders them, at 16, 20, 24,
#     32 and 48 px (IShellItemImageFactory, icon only), saved as <app>-<size>.png;
#   - the taskbar strip itself (CopyFromScreen at native resolution) when it is on screen,
#     saved as taskbar-capture.png; with an auto-hidden taskbar only the 2 px sliver at the
#     bottom edge is visible, so its colour is sampled instead (taskbar-colour.txt).
# taskbar_sheet.py draws the strips in flat taskbar colours; the capture or the sampled
# colour is there to check those colours against the real taskbar.
# Usage: powershell -ExecutionPolicy Bypass -File taskbar-icons.ps1 -Out <dir>
param([Parameter(Mandatory = $true)][string]$Out)
$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System; using System.Drawing; using System.Drawing.Imaging; using System.Runtime.InteropServices;
public static class TB {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IShellItemImageFactory { [PreserveSig] int GetImage(SIZE size, int flags, out IntPtr phbm); }
  [StructLayout(LayoutKind.Sequential)] struct SIZE { public int cx, cy; }
  [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
  static extern void SHCreateItemFromParsingName(string path, IntPtr pbc, [MarshalAs(UnmanagedType.LPStruct)] Guid riid, out IShellItemImageFactory f);
  [StructLayout(LayoutKind.Sequential)] struct BITMAP { public int bmType, bmWidth, bmHeight, bmWidthBytes; public ushort bmPlanes, bmBitsPixel; public IntPtr bmBits; }
  [DllImport("gdi32.dll")] static extern int GetObject(IntPtr h, int c, out BITMAP b);
  [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr h);
  // Shell icon at an exact pixel size; the DIB is 32 bpp, bottom-up, premultiplied alpha.
  public static Bitmap Icon(string path, int px) {
    IShellItemImageFactory f;
    SHCreateItemFromParsingName(path, IntPtr.Zero, new Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), out f);
    IntPtr hbm; SIZE s; s.cx = px; s.cy = px;
    int hr = f.GetImage(s, 0x4 /* SIIGBF_ICONONLY */, out hbm);
    if (hr != 0) throw new Exception("GetImage " + hr.ToString("x"));
    BITMAP bm; GetObject(hbm, Marshal.SizeOf(typeof(BITMAP)), out bm);
    var outb = new Bitmap(bm.bmWidth, bm.bmHeight, PixelFormat.Format32bppArgb);
    var row = new byte[bm.bmWidthBytes];
    for (int y = 0; y < bm.bmHeight; y++) {
      Marshal.Copy(bm.bmBits + y * bm.bmWidthBytes, row, 0, row.Length);
      int dy = bm.bmHeight - 1 - y;
      for (int x = 0; x < bm.bmWidth; x++) {
        int a = row[x * 4 + 3];
        Func<int, int> un = c => a == 0 ? 0 : Math.Min(255, (c * 255 + a / 2) / a);
        outb.SetPixel(x, dy, Color.FromArgb(a, un(row[x * 4 + 2]), un(row[x * 4 + 1]), un(row[x * 4])));
      }
    }
    DeleteObject(hbm);
    return outb;
  }
}
"@
[TB]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
Add-Type -AssemblyName System.Windows.Forms
New-Item -ItemType Directory -Force $Out | Out-Null

$pinned = Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar'
foreach ($lnk in Get-ChildItem $pinned -Filter *.lnk) {
  $name = ($lnk.BaseName -replace '[^A-Za-z0-9]+', '-').ToLower()
  foreach ($px in 16, 20, 24, 32, 48) {
    $bmp = [TB]::Icon($lnk.FullName, $px)
    $bmp.Save((Join-Path $Out "$name-$px.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
  }
  "icon: $name"
}

$scr = [System.Windows.Forms.Screen]::PrimaryScreen
$b = $scr.Bounds; $w = $scr.WorkingArea
$h = $b.Bottom - $w.Bottom
if ($h -gt 0) {
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.Left, $w.Bottom, 0, 0, $bmp.Size)
  $bmp.Save((Join-Path $Out 'taskbar-capture.png'), [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  "captured taskbar strip ${h} px tall"
} else {
  # Auto-hide: sample the sliver the hidden taskbar leaves at the bottom edge, away from the ends.
  $bmp = New-Object System.Drawing.Bitmap 400, 1
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen([int]($b.Width * 0.6), $b.Bottom - 1, 0, 0, $bmp.Size)
  $px = 0..399 | ForEach-Object { $bmp.GetPixel($_, 0) }
  $c = $px | Group-Object { '#{0:x2}{1:x2}{2:x2}' -f $_.R, $_.G, $_.B } | Sort-Object Count -Descending | Select-Object -First 1
  $g.Dispose(); $bmp.Dispose()
  $c.Name | Set-Content -Encoding ascii (Join-Path $Out 'taskbar-colour.txt')
  "taskbar is auto-hidden; sampled edge colour $($c.Name) ($($c.Count)/400 px)"
}
$t = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
"theme: SystemUsesLightTheme=$($t.SystemUsesLightTheme)"
