param(
    [string]$Doc = "",
    [string]$Out = "shot.png",
    [int]$ClickX = -1,
    [int]$ClickY = -1,
    [int]$Click2X = -1,
    [int]$Click2Y = -1,
    [string]$TypeText = "",
    [string]$Steps = "",
    [int]$WaitMs = 1800,
    # Which binary to photograph. Empty means the debug build.
    [string]$Exe = ""
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint from, uint to, bool attach);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
}
"@

# Windows only hands the foreground to a process that already owns it, so a
# freshly started window can come up behind the terminal and never see the
# keystrokes the script sends. Sharing the input queue with the current
# foreground thread lifts that lock for the duration of the call.
function Set-Foreground([IntPtr]$target) {
    $fg = [W]::GetForegroundWindow()
    $mine = [W]::GetWindowThreadProcessId($target, [IntPtr]::Zero)
    $theirs = [W]::GetWindowThreadProcessId($fg, [IntPtr]::Zero)
    if ($theirs -ne 0 -and $theirs -ne $mine) {
        [void][W]::AttachThreadInput($theirs, $mine, $true)
    }
    [void][W]::ShowWindow($target, 5)
    [void][W]::BringWindowToTop($target)
    [void][W]::SetForegroundWindow($target)
    if ($theirs -ne 0 -and $theirs -ne $mine) {
        [void][W]::AttachThreadInput($theirs, $mine, $false)
    }
}

$exe = if ($Exe -ne "") { $Exe } else { Join-Path (Split-Path -Parent $PSScriptRoot) "target\debug\meowdown.exe" }
# An instance left over from an interrupted run keeps drawing the same file, so
# the click would land on both windows and the shot could be either one.
Get-Process meowdown -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 300
if ($Doc -ne "") {
    $proc = Start-Process -FilePath $exe -ArgumentList "`"$Doc`"" -PassThru
} else {
    $proc = Start-Process -FilePath $exe -PassThru
}
Start-Sleep -Milliseconds $WaitMs
$proc.Refresh()
$h = $proc.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { Write-Error "no window"; exit 1 }
Set-Foreground $h
Start-Sleep -Milliseconds 500
Write-Output "foreground ok: $([W]::GetForegroundWindow() -eq $h)"

$origin = New-Object W+POINT
[void][W]::ClientToScreen($h, [ref]$origin)
Write-Output "client origin $($origin.X),$($origin.Y)"

# Grabs the window right now. Used by the `snap:` step to catch a frame in the
# middle of an animation instead of only the settled end state.
function Save-Shot([string]$path) {
    $r = New-Object W+RECT
    [void][W]::GetWindowRect($h, [ref]$r)
    $bmp = New-Object System.Drawing.Bitmap(($r.Right - $r.Left), ($r.Bottom - $r.Top))
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Write-Output "snapped $path"
}

# Clicks land on whatever is in front, and the editor that started this script
# can take the foreground back while we are waiting, so every click asks for it
# again first. Without this the pointer moves and the button goes down on the
# terminal instead of the window being photographed.
function Invoke-Click([int]$x, [int]$y, [int]$settle) {
    Set-Foreground $h
    Start-Sleep -Milliseconds 120
    [void][W]::SetCursorPos($origin.X + $x, $origin.Y + $y)
    Start-Sleep -Milliseconds 200
    [W]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 60
    [W]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds $settle
}

if ($ClickX -ge 0) {
    Invoke-Click $ClickX $ClickY 600
}

if ($Click2X -ge 0) {
    Invoke-Click $Click2X $Click2Y 800
}

if ($TypeText -ne "") {
    Start-Sleep -Milliseconds 300
    Add-Type -AssemblyName System.Windows.Forms
    [System.Windows.Forms.SendKeys]::SendWait($TypeText)
    Start-Sleep -Milliseconds 800
}

# A tiny script of interactions, so one run can click, type, click elsewhere and
# press Ctrl+Z before the screenshot: "click:400,220;keys:ABC;wait:300;keys:^z".
if ($Steps -ne "") {
    Add-Type -AssemblyName System.Windows.Forms
    foreach ($step in $Steps.Split(";")) {
        # Only the name is trimmed: a `paste:` payload may legitimately start or
        # end with the space that makes `# ` a heading.
        $step = $step.TrimStart()
        if ($step -eq "") { continue }
        $pair = $step.Split(":", 2)
        $pair[0] = $pair[0].Trim()
        switch ($pair[0]) {
            "click" {
                $xy = $pair[1].Split(",")
                Invoke-Click ([int]$xy[0]) ([int]$xy[1]) 450
            }
            "keys" {
                Set-Foreground $h
                [System.Windows.Forms.SendKeys]::SendWait($pair[1])
                Start-Sleep -Milliseconds 450
            }
            "ctrlclick" {
                # A link only opens with the modifier held, which SendKeys cannot
                # do around a mouse event, so Ctrl is pressed at the driver level.
                $xy = $pair[1].Split(",")
                Set-Foreground $h
                Start-Sleep -Milliseconds 120
                [void][W]::SetCursorPos($origin.X + [int]$xy[0], $origin.Y + [int]$xy[1])
                Start-Sleep -Milliseconds 200
                [W]::keybd_event(0x11, 0, 0, [IntPtr]::Zero)
                Start-Sleep -Milliseconds 80
                [W]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
                Start-Sleep -Milliseconds 60
                [W]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
                Start-Sleep -Milliseconds 80
                [W]::keybd_event(0x11, 0, 0x0002, [IntPtr]::Zero)
                Start-Sleep -Milliseconds 900
            }
            "paste" {
                # SendKeys goes through the input method, which swallows Latin
                # letters; putting the text on the clipboard first does not.
                Set-Foreground $h
                Set-Clipboard -Value $pair[1]
                Start-Sleep -Milliseconds 150
                [System.Windows.Forms.SendKeys]::SendWait("^v")
                Start-Sleep -Milliseconds 500
            }
            "image" {
                # Puts a bitmap on the clipboard, which is what a screenshot
                # taken with Win+Shift+S looks like to the editor.
                Add-Type -AssemblyName System.Drawing
                $bitmap = [System.Drawing.Image]::FromFile($pair[1])
                [System.Windows.Forms.Clipboard]::SetImage($bitmap)
                $bitmap.Dispose()
                Start-Sleep -Milliseconds 200
            }
            "wait" {
                Start-Sleep -Milliseconds ([int]$pair[1])
            }
            "snap" {
                Save-Shot $pair[1]
            }
            "press" {
                # Click with no settle time, so the next `snap:` lands on an
                # animation that is still in flight.
                $xy = $pair[1].Split(",")
                Invoke-Click ([int]$xy[0]) ([int]$xy[1]) 0
            }
            "size" {
                # Resize the window without moving it, so a layout can be
                # checked at a second width in the same run.
                $wh = $pair[1].Split(",")
                [void][W]::SetWindowPos($h, [IntPtr]::Zero, 0, 0, [int]$wh[0], [int]$wh[1], 0x0006)
                Start-Sleep -Milliseconds 600
            }
            "drag" {
                # Press at the first point, walk the pointer to the second in a
                # few steps — a selection only grows if the moves arrive while
                # the button is down — then release.
                $p = $pair[1].Split(",")
                Set-Foreground $h
                Start-Sleep -Milliseconds 120
                [void][W]::SetCursorPos($origin.X + [int]$p[0], $origin.Y + [int]$p[1])
                Start-Sleep -Milliseconds 200
                [W]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
                Start-Sleep -Milliseconds 120
                $steps = 8
                for ($i = 1; $i -le $steps; $i++) {
                    $nx = [int]$p[0] + [int](([int]$p[2] - [int]$p[0]) * $i / $steps)
                    $ny = [int]$p[1] + [int](([int]$p[3] - [int]$p[1]) * $i / $steps)
                    [void][W]::SetCursorPos($origin.X + $nx, $origin.Y + $ny)
                    Start-Sleep -Milliseconds 40
                }
                Start-Sleep -Milliseconds 120
                [W]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
                Start-Sleep -Milliseconds 400
            }
            "wheel" {
                # Negative scrolls down. The cursor has to be over the document.
                $at = $pair[1].Split(",")
                [void][W]::SetCursorPos($origin.X + [int]$at[0], $origin.Y + [int]$at[1])
                Start-Sleep -Milliseconds 150
                $delta = [int]$at[2]
                # Negative deltas have to be handed over as their two's
                # complement, which is what the cast to uint32 does.
                [W]::mouse_event(0x0800, 0, 0, [uint32]([int64]$delta -band 0xFFFFFFFFL), [IntPtr]::Zero)
                Start-Sleep -Milliseconds 400
            }
        }
    }
    Start-Sleep -Milliseconds 500
}

$r = New-Object W+RECT
[void][W]::GetWindowRect($h, [ref]$r)
$w = $r.Right - $r.Left
$hgt = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap($w, $hgt)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $Out ($w x $hgt) at $($r.Left),$($r.Top)"
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
