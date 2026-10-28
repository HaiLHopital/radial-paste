$ErrorActionPreference = 'Stop'
$env:RUST_LOG = 'warn,radial=debug,wgpu=off,wgpu_hal=off'
$exe = Join-Path $PSScriptRoot 'target\debug\radial-paste.exe'
$out = Join-Path $PSScriptRoot 'smoke-out.log'
$err = Join-Path $PSScriptRoot 'smoke-err.log'
Remove-Item $out, $err -ErrorAction SilentlyContinue

Add-Type -Namespace W -Name U -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
'@

$p = Start-Process -FilePath $exe -PassThru -RedirectStandardOutput $out -RedirectStandardError $err -WindowStyle Hidden
Start-Sleep -Milliseconds 2500

# Hold the hotkey: Ctrl+Shift+Space down.
[W.U]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)   # Ctrl down
[W.U]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero)   # Shift down
[W.U]::keybd_event(0x20, 0, 0, [UIntPtr]::Zero)   # Space down
Start-Sleep -Milliseconds 150

# Flick right.
[W.U]::mouse_event(0x0001, 300, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 60

# Let go: Space up, Shift up, Ctrl up (KEYEVENTF_KEYUP = 2).
[W.U]::keybd_event(0x20, 0, 2, [UIntPtr]::Zero)
[W.U]::keybd_event(0x10, 0, 2, [UIntPtr]::Zero)
[W.U]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 700

Stop-Process -Id $p.Id -Force
Get-Content $err
